use rustfft::{Fft, FftPlanner, num_complex::Complex64};
use std::{
	f64::consts::PI,
	fmt::{self, Debug},
	sync::Arc,
	time::Duration,
};

use crate::audio::SampleReader;
use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumGenerator};

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
	pub dft_window_size: usize,
	/// The part of each DFT window that repeats the one before it, as a
	/// fraction of the window.
	///
	/// What is left is the hop: the audio the spectrum advances by per tick,
	/// and so the tick interval itself. At 0.5 each window shares half its
	/// samples with the last and the spectrum advances twice per window.
	pub overlap: f64,
}

/// The audio one tick advances by, in samples: the part of a window that does
/// not repeat the one before it.
pub fn hop_samples(dft_window_size: usize, overlap: f64) -> f64 {
	dft_window_size as f64 * (1.0 - overlap)
}

/// The DFT window a grid of `samples_per_octave` bins needs.
///
/// A window of `n` samples spaces its bins `sample_rate / n` apart, evenly,
/// while the grid's bins crowd together towards the low end. So the octave
/// below the Nyquist frequency is where the grid is coarsest and the transform
/// has the best chance of resolving it: the grid's step there is a
/// `2^(1/samples_per_octave) - 1` fraction of `sample_rate / 4`, and matching it
/// takes `4 / (2^(1/samples_per_octave) - 1)` samples. The sample rate cancels,
/// so the answer holds at any rate. Lower octaves the transform interpolates
/// across, which no window short enough to be worth planning would fix.
///
/// `rustfft` plans any size but is fastest at a power of two, so the window
/// rounds up to one.
pub fn dft_window_size(samples_per_octave: usize) -> usize {
	let samples = 4.0 / (2f64.powf(1.0 / samples_per_octave as f64) - 1.0);
	(samples.ceil() as usize).next_power_of_two()
}

pub struct AudioSpectrumGenerator {
	audio_buffer: SampleReader,
	/// The window the ring is read into, one DFT window long. Held across ticks
	/// so that the spectrum thread allocates nothing per tick.
	samples: Vec<f32>,
	analyzer: Analyzer,
	overlap: f64,
	/// The overrun count as of the last tick that logged one. The counter itself
	/// only ever grows; this is what turns it into a per-tick delta.
	reported_overruns: u64,
}

impl Debug for AudioSpectrumGenerator {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("AudioSpectrumGenerator")
			.field("analyzer", &self.analyzer)
			.field("overlap", &self.overlap)
			.finish()
	}
}

impl AudioSpectrumGenerator {
	pub fn new(config: Config, audio_buffer: SampleReader, sample_rate: u32) -> Self {
		let mut generator = AudioSpectrumGenerator {
			audio_buffer,
			samples: Vec::new(),
			analyzer: Analyzer::new(WindowShape::Hann, sample_rate),
			overlap: config.overlap,
			reported_overruns: 0,
		};
		generator.set_window_size(config.dft_window_size);
		generator
	}

	/// Applies `config`, keeping the DFT plan when the window size is the one
	/// already planned for.
	pub fn set_config(&mut self, config: Config) {
		if config.dft_window_size != self.analyzer.window_size() {
			self.set_window_size(config.dft_window_size);
		}
		self.overlap = config.overlap;
	}

	pub fn set_window_size(&mut self, dft_window_size: usize) {
		self.analyzer.set_window_size(dft_window_size);
		self.samples.resize(dft_window_size, 0.0);
	}

	/// Log any audio the real-time thread dropped since the last tick.
	///
	/// The ring holds ~0.7 s of audio and this thread drains it every tick, so an
	/// overrun means the spectrum thread stalled long enough to lose sound. That
	/// is a starved pipeline, not a quiet one, and nothing else would show it.
	fn report_overruns(&mut self) {
		let overruns = self.audio_buffer.overruns();
		if overruns > self.reported_overruns {
			log::warn!(
				"audio ring overrun: {} samples dropped ({} since startup)",
				overruns - self.reported_overruns,
				overruns,
			);
			self.reported_overruns = overruns;
		}
	}
}

impl SpectrumGenerator for AudioSpectrumGenerator {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		// Nothing to do if buffer has no samples.
		if buffer.params().samples() == 0 {
			return buffer.fill(|_, _| ());
		}

		self.report_overruns();

		let read = self.audio_buffer.read_latest(&mut self.samples);
		self.analyzer
			.run_dft(self.samples[..read].iter().map(|&sample| sample as f64));
		self.analyzer.fill_bins(buffer)
	}

	/// The time the hop covers: the audio between one window and the next.
	fn interval(&self) -> Duration {
		let hop = hop_samples(self.analyzer.window_size(), self.overlap);
		Duration::from_secs_f64(hop / self.analyzer.sample_rate as f64)
	}

	fn set_sample_rate(&mut self, sample_rate: u32) {
		self.analyzer.set_sample_rate(sample_rate);
	}
}

/// The shape the samples are tapered to before the DFT, to keep a frequency
/// between two bins from leaking across the whole spectrum.
#[derive(Debug)]
pub enum WindowShape {
	Hann,
}

impl WindowShape {
	pub fn generate(&self, xs: &mut [f64]) {
		let size = xs.len();
		match self {
			Self::Hann => {
				for (i, x) in xs.iter_mut().enumerate() {
					*x = (PI * i as f64 / (size - 1) as f64).sin().powi(2);
				}
			}
		}
	}
}

/// The DFT half of the generator: windowing and the transform, then folding the
/// output into log-spaced power bins.
///
/// The two steps are separate because only one of them is ours — the transform
/// is `rustfft`'s — and they convert different things: samples to complex bins,
/// then complex bins to log-spaced ones.
pub struct Analyzer {
	window_shape: WindowShape,
	sample_rate: u32,
	dft: Arc<dyn Fft<f64>>,
	dft_window: Vec<Complex64>,
	windowing: Vec<f64>,
}

impl Analyzer {
	/// An analyzer with no window size yet; call
	/// [`set_window_size`](Self::set_window_size) before using it.
	pub fn new(window_shape: WindowShape, sample_rate: u32) -> Self {
		Analyzer {
			window_shape,
			sample_rate,
			dft: FftPlanner::new().plan_fft_forward(1),
			dft_window: Vec::new(),
			windowing: Vec::new(),
		}
	}

	/// The number of samples one DFT covers.
	pub fn window_size(&self) -> usize {
		self.dft_window.len()
	}

	pub fn set_window_size(&mut self, dft_window_size: usize) {
		self.dft = FftPlanner::new().plan_fft_forward(dft_window_size);
		self.dft_window = vec![Complex64::new(0.0, 0.0); dft_window_size];
		self.windowing = vec![0.0; dft_window_size];
		self.window_shape.generate(&mut self.windowing);
	}

	pub fn set_sample_rate(&mut self, sample_rate: u32) {
		self.sample_rate = sample_rate;
	}

	/// Tapers `samples` to the window shape and runs the forward DFT over them,
	/// leaving the result in the analyzer.
	///
	/// Samples beyond the window size are ignored; a short iterator leaves the
	/// tail of the previous window in place.
	pub fn run_dft(&mut self, samples: impl Iterator<Item = f64>) {
		let windowed_samples = samples.zip(self.windowing.iter()).map(|(a, &b)| a * b);

		for (sample, dst) in windowed_samples.zip(self.dft_window.iter_mut()) {
			*dst = Complex64::new(sample, 0.0);
		}

		self.dft.process(&mut self.dft_window);
	}

	/// Folds the DFT output into `buffer`'s log-spaced power bins.
	///
	/// # Preconditions
	/// - [`run_dft`](Self::run_dft) has been called since the window size last
	///   changed.
	pub fn fill_bins(&self, buffer: SpectrumBuffer) -> Spectrum {
		let n = self.dft_window.len();

		buffer.fill(|spectrum, spectrum_params| {
			// Use slice::fill when stable.
			// https://github.com/rust-lang/rust/issues/70758
			spectrum.iter_mut().for_each(|val| *val = 0.0);

			let log_freqs = spectrum_params.log_frequencies();
			let mut i = 1; // i indexes into spectrum

			// DFT applied to real values is even symmetric, meaning values n / 2 + 1, .., n - 1
			// are complex conjugates of samples 1, .., n / 2 - 1. Looked at another way, the DFT
			// output folds around the Nyquist frequency, so only take values below the Nyquist
			// frequency. Also skip the DC component because it has no log frequency.
			for j in 1..(n / 2) {
				let dft_out_freq = (self.sample_rate as f64 * j as f64) / n as f64;
				let dft_out_log_freq = dft_out_freq.log2();

				// Advance j until log_freqs[i - 1] <= dft_out_log_freq.
				if log_freqs[i - 1] > dft_out_log_freq {
					continue;
				}

				// Advance i until dft_out_log_freq < log_freqs[i].
				while dft_out_log_freq >= log_freqs[i] {
					i += 1;
					if i >= log_freqs.len() {
						return;
					}
				}

				let dft_out_val = dft_out_to_val(&self.dft_window[j], n);
				let interp_ratio =
					(dft_out_log_freq - log_freqs[i - 1]) / (log_freqs[i] - log_freqs[i - 1]);
				spectrum[i - 1] += (1.0 - interp_ratio) * dft_out_val;
				spectrum[i] += interp_ratio * dft_out_val;
			}
		})
	}
}

// Parseval's theorem states that power, proportional to square of amplitude, is preserved
// under FFT. Therefore, we need the interpolations to split power, not amplitude. The division
// by N on the coefficient appears in the IFFT formula.
fn dft_out_to_val(dft_out: &Complex64, n: usize) -> f64 {
	(dft_out / n as f64).norm_sqr()
}

impl Debug for Analyzer {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("Analyzer")
			.field("window_shape", &self.window_shape)
			.field("sample_rate", &self.sample_rate)
			.field("window_size", &self.window_size())
			.finish()
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use rand::{RngExt, SeedableRng, rngs::StdRng};

	use crate::spectrum::SpectrumParams;
	use crate::test_support::{sample_reader, sine_wave, spectrum_params};

	const SAMPLE_RATE: u32 = 48_000;
	const WINDOW: usize = 2048;
	const OVERLAP: f64 = 0.5;

	/// A generator over `samples`, at the default window size and sample rate.
	fn generator(samples: &[f32]) -> AudioSpectrumGenerator {
		AudioSpectrumGenerator::new(
			Config {
				dft_window_size: WINDOW,
				overlap: OVERLAP,
			},
			sample_reader(samples),
			SAMPLE_RATE,
		)
	}

	/// The spectrum of `components`, on a grid of `samples` bins.
	fn analyze(components: &[(f64, f64)], samples: usize) -> Spectrum {
		let params = spectrum_params(samples);
		let signal = sine_wave(components, SAMPLE_RATE, WINDOW);
		generator(&signal).generate(SpectrumBuffer::new(params))
	}

	/// The index of the largest value in the spectrum.
	fn peak_index(spectrum: &Spectrum) -> usize {
		spectrum
			.values()
			.iter()
			.enumerate()
			.max_by(|(_, left), (_, right)| left.partial_cmp(right).expect("no value is NaN"))
			.expect("the spectrum has at least one bin")
			.0
	}

	/// The width of a DFT bin: the finest frequency the analysis resolves.
	const DFT_BIN_HZ: f64 = SAMPLE_RATE as f64 / WINDOW as f64;

	#[test]
	fn a_sine_lands_in_the_bin_for_its_frequency() {
		// Across the range, because the log grid is much finer than a DFT bin
		// at the bottom of it and about as fine at the top.
		for frequency in [250.0, 440.0, 5000.0] {
			let spectrum = analyze(&[(frequency, 1.0)], 1196);
			let peak = spectrum.params().frequencies()[peak_index(&spectrum)];

			assert!(
				(peak - frequency).abs() <= DFT_BIN_HZ,
				"a {frequency} Hz sine peaked at {peak} Hz, more than one DFT bin away",
			);
		}
	}

	#[test]
	fn binning_preserves_the_power_below_nyquist() {
		let signal = sine_wave(&[(1000.0, 1.0)], SAMPLE_RATE, WINDOW);
		let spectrum = generator(&signal).generate(SpectrumBuffer::new(spectrum_params(1196)));

		// The generator windows the samples before the DFT, so the power to
		// account for is the windowed signal's.
		let windowed_power = signal
			.iter()
			.enumerate()
			.map(|(index, &sample)| {
				let window = (PI * index as f64 / (WINDOW - 1) as f64).sin().powi(2);
				(sample as f64 * window).powi(2)
			})
			.sum::<f64>()
			/ WINDOW as f64;
		let binned_power = spectrum.values().iter().sum::<f64>();

		// Half, because the DFT of a real signal is symmetric about the Nyquist
		// frequency and the binning loop takes only the lower half. Counting the
		// mirror image as well would double this.
		let expected = windowed_power / 2.0;
		assert!(
			(binned_power / expected - 1.0).abs() < 0.01,
			"binned {binned_power}, expected about {expected}",
		);
	}

	#[test]
	fn power_is_split_between_the_bins_a_frequency_falls_between() {
		// Two octaves per bin, so the tone sits far from either bin centre and
		// the split is not rounding.
		let params = Arc::new(SpectrumParams::exp_spaced(3, 200.0, 3200.0));
		// A quarter of the way from 200 Hz to 800 Hz in log frequency.
		let frequency = (200.0f64)
			.log2()
			.mul_add(0.75, (800.0f64).log2() * 0.25)
			.exp2();

		let signal = sine_wave(&[(frequency, 1.0)], SAMPLE_RATE, WINDOW);
		let spectrum = generator(&signal).generate(SpectrumBuffer::new(params));

		let values = spectrum.values();
		let lower_share = values[0] / (values[0] + values[1]);
		assert!(
			(lower_share - 0.75).abs() < 0.05,
			"the nearer bin should take three quarters of the power, took {lower_share}",
		);
		assert!(
			values[2] < 1e-9,
			"no power belongs two octaves above the tone",
		);
	}

	#[test]
	fn dc_has_no_log_frequency_and_reaches_no_bin() {
		let spectrum = generator(&[1.0; WINDOW]).generate(SpectrumBuffer::new(spectrum_params(64)));

		// A constant signal carries its power at DC, which the binning loop
		// skips because zero has no logarithm. What is left is the window's own
		// numerical leakage, orders of magnitude below the signal.
		let binned_power = spectrum.values().iter().sum::<f64>();
		let signal_power = 1.0;
		assert!(
			binned_power / signal_power < 1e-9,
			"a constant signal put {binned_power} into the spectrum",
		);
	}

	#[test]
	fn silence_produces_an_empty_spectrum() {
		let spectrum = analyze(&[], 1196);

		assert!(spectrum.values().iter().all(|&value| value == 0.0));
	}

	#[test]
	fn a_spectrum_with_no_bins_is_returned_untouched() {
		let signal = sine_wave(&[(440.0, 1.0)], SAMPLE_RATE, WINDOW);

		let spectrum = generator(&signal).generate(SpectrumBuffer::default());

		assert_eq!(spectrum.values(), []);
	}

	/// The seconds of audio one window covers.
	fn window_seconds(window: usize, sample_rate: u32) -> f64 {
		window as f64 / sample_rate as f64
	}

	/// How far apart two of these durations may be and still count as equal.
	/// A `Duration` holds whole nanoseconds, so a tick that divides a window
	/// unevenly does not multiply back to it exactly.
	const TOLERANCE_SECONDS: f64 = 1e-6;

	#[test]
	fn the_overlap_says_how_many_ticks_a_window_takes() {
		// A window advances in one tick with nothing overlapping, in two when
		// half of it repeats, in four when three quarters do.
		for (overlap, ticks) in [(0.0, 1.0), (0.5, 2.0), (0.75, 4.0)] {
			for window in [512, 1024, 2048, 4096] {
				let mut generator = generator(&[]);
				generator.set_config(Config {
					dft_window_size: window,
					overlap,
				});

				let ticked = generator.interval().as_secs_f64() * ticks;
				let expected = window_seconds(window, SAMPLE_RATE);
				assert!(
					(ticked - expected).abs() < TOLERANCE_SECONDS,
					"a {window}-sample window at {overlap} overlap took {ticked} s \
					 to advance, expected {expected} s",
				);
			}
		}
	}

	#[test]
	fn the_window_resolves_the_grid_in_the_octave_below_nyquist() {
		for samples_per_octave in [12, 60, 180, 360, 720] {
			let window = dft_window_size(samples_per_octave);
			assert!(
				window.is_power_of_two(),
				"{samples_per_octave} bins / octave planned a {window}-sample window",
			);

			// Both steps as a fraction of the sample rate, which cancels: the
			// DFT's is 1 / window, and the grid's at the Nyquist frequency's
			// lower octave is that octave's bottom, a quarter of the rate,
			// times the ratio between two grid bins.
			let dft_step = 1.0 / window as f64;
			let grid_step = 0.25 * (2f64.powf(1.0 / samples_per_octave as f64) - 1.0);
			assert!(
				dft_step <= grid_step,
				"a {window}-sample window steps by {dft_step} of the sample rate, \
				 past the {grid_step} between two of {samples_per_octave} bins / octave",
			);
			assert!(
				dft_step * 2.0 > grid_step,
				"a {window}-sample window is twice what \
				 {samples_per_octave} bins / octave asks for",
			);
		}
	}

	#[test]
	fn changing_the_sample_rate_updates_the_tick_interval() {
		let mut generator = generator(&[]);
		generator.set_sample_rate(96_000);

		let ticked = generator.interval().as_secs_f64() / (1.0 - OVERLAP);
		let expected = window_seconds(WINDOW, 96_000);
		assert!(
			(ticked - expected).abs() < TOLERANCE_SECONDS,
			"{ticked} s, expected {expected} s"
		);
	}

	#[test]
	fn changing_the_sample_rate_updates_frequency_binning() {
		let signal = sine_wave(&[(440.0, 1.0)], 96_000, WINDOW);
		let mut generator = generator(&signal);
		generator.set_sample_rate(96_000);

		let spectrum = generator.generate(SpectrumBuffer::new(spectrum_params(1196)));
		let peak = spectrum.params().frequencies()[peak_index(&spectrum)];
		let dft_bin_hz = 96_000.0 / WINDOW as f64;

		assert!((peak - 440.0).abs() <= dft_bin_hz);
	}

	#[test]
	fn fft_preserves_power() {
		let n = 2048;

		let mut input = vec![0.0; n];

		let mut rng = StdRng::seed_from_u64(0);
		for x in input.iter_mut() {
			*x = rng.random::<u32>() as f64;
		}

		let dft = FftPlanner::new().plan_fft_forward(n);
		let mut dft_buffer = input
			.iter()
			.map(|&val| Complex64::new(val, 0.0))
			.collect::<Vec<_>>();
		dft.process(&mut dft_buffer);

		let input_power = input.iter().map(|x| x * x).sum::<f64>() / n as f64;
		let output_power = dft_buffer.iter().map(|x| dft_out_to_val(x, n)).sum::<f64>();
		assert!(input_power / output_power > 0.99999 && input_power / output_power < 1.00001);
	}
}
