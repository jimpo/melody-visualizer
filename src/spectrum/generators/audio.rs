use rustfft::{Fft, FftPlanner, num_complex::Complex64};
use std::{
	f64::consts::PI,
	fmt::{self, Debug},
	sync::Arc,
	time::Duration,
};

use crate::audio::SampleReader;
use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumGenerator};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Config {
	/// The audio one DFT covers, in milliseconds. The sample count follows the
	/// sample rate, so a rate JACK changes keeps the window the same length in
	/// time.
	pub window_ms: u32,
	/// The spectra produced per second, independent of the window. A window
	/// longer than the hop repeats part of the last; a shorter one skips audio.
	pub update_rate: u32,
}

impl Default for Config {
	fn default() -> Self {
		Config {
			window_ms: 50,
			update_rate: 50,
		}
	}
}

/// The shortest window the Window slider offers, in milliseconds.
pub const MIN_WINDOW_MS: u32 = 10;

/// The longest window the slider offers, and the capture ring holds.
pub const MAX_WINDOW_MS: u32 = 1000;

/// The slowest update rate the Update rate slider offers, in spectra per
/// second. The capture ring holds the hop it leaves.
pub const MIN_UPDATE_RATE: u32 = 1;

/// The fastest update rate the slider offers, and so the shortest tick the
/// chain has to fit in.
pub const MAX_UPDATE_RATE: u32 = 100;

/// The samples a window of `window_ms` holds at `sample_rate`.
pub fn window_samples(window_ms: u32, sample_rate: u32) -> usize {
	(window_ms as f64 * sample_rate as f64 / 1000.0).round() as usize
}

pub struct AudioSpectrumGenerator {
	audio_buffer: SampleReader,
	/// The window the ring is read into, one DFT window long. Held across ticks
	/// so that the spectrum thread allocates nothing per tick.
	samples: Vec<f32>,
	analyzer: Analyzer,
	config: Config,
	/// The overrun count as of the last tick that logged one. The counter itself
	/// only ever grows; this is what turns it into a per-tick delta.
	reported_overruns: u64,
}

impl Debug for AudioSpectrumGenerator {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("AudioSpectrumGenerator")
			.field("analyzer", &self.analyzer)
			.field("config", &self.config)
			.finish()
	}
}

impl AudioSpectrumGenerator {
	pub fn new(config: Config, audio_buffer: SampleReader, sample_rate: u32) -> Self {
		let mut generator = AudioSpectrumGenerator {
			audio_buffer,
			samples: Vec::new(),
			analyzer: Analyzer::new(WindowShape::Hann, sample_rate),
			config,
			reported_overruns: 0,
		};
		generator.plan_window();
		generator
	}

	/// Applies `config`, keeping the DFT plan when the window holds the samples
	/// already planned for.
	pub fn set_config(&mut self, config: Config) {
		self.config = config;
		self.plan_window();
	}

	/// Sizes the DFT to the configured window at the current sample rate,
	/// re-planning only when the sample count changes.
	fn plan_window(&mut self) {
		let size = window_samples(self.config.window_ms, self.analyzer.sample_rate);
		if size != self.analyzer.window_size() {
			self.analyzer.set_window_size(size);
			self.samples.resize(size, 0.0);
		}
	}

	/// Log any audio the real-time thread dropped since the last tick.
	///
	/// The ring holds the longest window and the slowest hop at the highest
	/// sample rate, and this thread drains it every tick, so an overrun means
	/// the spectrum thread stalled long enough to lose sound. That is a starved
	/// pipeline, not a quiet one, and nothing else would show it.
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

	/// The time between two spectra: one over the update rate.
	fn interval(&self) -> Duration {
		// A persisted rate of zero would ask for an infinite interval.
		Duration::from_secs_f64(1.0 / self.config.update_rate.max(1) as f64)
	}

	fn set_sample_rate(&mut self, sample_rate: u32) {
		self.analyzer.set_sample_rate(sample_rate);
		self.plan_window();
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
/// output into log-spaced amplitude bins.
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

	/// Folds the DFT output into `buffer`'s log-spaced bins, as amplitude.
	///
	/// The binning splits and sums power, because the powers of incoherent
	/// components add and their amplitudes do not. Each bin takes the square
	/// root of its power last, so a partial at half the amplitude of another is
	/// drawn at half its value.
	///
	/// Each DFT bin spreads its power as a triangle in log frequency, peaking at
	/// the bin's own frequency and reaching zero at its two neighbours'. Every
	/// triangle carries exactly its bin's power, so the total is preserved, and
	/// they overlap, so no output bin between two DFT bins is left empty. Where
	/// the grid is the coarser of the two the triangle is narrower than one
	/// output bin, and the same arithmetic gives the two nearest bins a linear
	/// split of the power.
	///
	/// A bin therefore sums power, not power per octave, and one DFT bin is
	/// worth tens of output bins at 200 Hz and less than one at 20 kHz. So a
	/// tone the analysis cannot place better than ±20 Hz is drawn as the wide,
	/// low hump that width deserves, and the same tone an octave up as a narrow,
	/// tall one. Spreading it is the point — a comb of spikes with empty bins
	/// between them claims a precision the window does not have — but it does
	/// mean a bass note reaches a lower peak than a treble note of equal energy.
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

			// Neither a grid too short to have a step nor a rate that gives the
			// DFT's bins no frequency has any bin to put power in.
			let log_freqs = spectrum_params.log_frequencies();
			if log_freqs.len() < 2 || self.sample_rate == 0 {
				return;
			}

			// The grid is exponentially spaced, so its log frequencies step by
			// a constant and an index past either end is arithmetic. The first
			// and last bins need one such neighbour to be weighed against.
			let base = log_freqs[0];
			let step = log_freqs[1] - base;
			let grid_log_freq = |i: isize| base + i as f64 * step;
			let last_bin = log_freqs.len() as isize - 1;

			let dft_log_freq = |j: usize| ((self.sample_rate as f64 * j as f64) / n as f64).log2();

			// DFT applied to real values is even symmetric, meaning values n / 2 + 1, .., n - 1
			// are complex conjugates of samples 1, .., n / 2 - 1. Looked at another way, the DFT
			// output folds around the Nyquist frequency, so only take values below the Nyquist
			// frequency. Also skip the DC component because it has no log frequency.
			for j in 1..(n / 2) {
				let peak = dft_log_freq(j);
				let right = dft_log_freq(j + 1);
				// DC has no log frequency to anchor the first bin's lower side,
				// so mirror its upper one. That bin sits at 20 Hz on the default
				// window, below any grid a listener would ask for.
				let left = if j == 1 {
					peak - (right - peak)
				} else {
					dft_log_freq(j - 1)
				};
				let power = dft_out_to_val(&self.dft_window[j], n);

				// Outside [left, right] the moment is a straight line, so a bin
				// clear of the triangle takes nothing however far the ends
				// reach. That makes a generous range safe and a tight one moot.
				let first = (((left - base) / step).floor() as isize).clamp(0, last_bin);
				let last = (((right - base) / step).ceil() as isize).clamp(0, last_bin);

				let mut previous = triangle_moment(grid_log_freq(first - 1), left, peak, right);
				let mut current = triangle_moment(grid_log_freq(first), left, peak, right);
				for i in first..=last {
					let next = triangle_moment(grid_log_freq(i + 1), left, peak, right);
					spectrum[i as usize] += power * (previous - 2.0 * current + next) / step;
					previous = current;
					current = next;
				}
			}

			// The second differences are never negative, but rounding can leave
			// a bin clear of every triangle a hair below zero.
			spectrum
				.iter_mut()
				.for_each(|val| *val = val.max(0.0).sqrt());
		})
	}
}

/// The triangle of unit area that rises from zero at `left`, peaks at `peak` and
/// falls to zero at `right`, integrated twice, at `x`.
///
/// An evenly spaced grid's bin covers a triangle of its own, one bin wide either
/// side, and the share of the power a bin takes is what the two triangles have
/// in common. Integrating twice is what turns that overlap into arithmetic: the
/// grid's triangle is the second difference of a straight line, so the overlap
/// is the second difference of this function across the bin and its neighbours,
/// divided by the grid step.
///
/// # Preconditions
/// - `left < peak < right`
fn triangle_moment(x: f64, left: f64, peak: f64, right: f64) -> f64 {
	let (rise, fall) = (peak - left, right - peak);
	let height = 2.0 / (rise + fall);

	if x <= left {
		0.0
	} else if x <= peak {
		height * (x - left).powi(3) / (6.0 * rise)
	} else if x <= right {
		// The rising side in full, then the falling side less the tail of it
		// still to come.
		height * rise.powi(2) / 6.0 + (x - peak)
			- height * (fall.powi(3) - (right - x).powi(3)) / (6.0 * fall)
	} else {
		// Past the triangle the area is fixed at one, so the integral of it
		// climbs by one for every step in `x`.
		height * (rise.powi(2) - fall.powi(2)) / 6.0 + fall + (x - right)
	}
}

// Parseval's theorem states that power, proportional to square of amplitude, is preserved
// under FFT. Therefore, we need the interpolations to split power, not amplitude; `fill_bins`
// takes the square root once the power is binned. The division by N on the coefficient appears
// in the IFFT formula.
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
	const WINDOW_MS: u32 = 50;
	/// The samples [`WINDOW_MS`] holds at [`SAMPLE_RATE`].
	const WINDOW: usize = 2400;

	/// A generator over `samples`, at the default window size and sample rate.
	fn generator(samples: &[f32]) -> AudioSpectrumGenerator {
		AudioSpectrumGenerator::new(
			Config {
				window_ms: WINDOW_MS,
				..Config::default()
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
	fn a_sine_leaves_no_gap_in_the_bins_it_reaches() {
		// The grid is 28 bins to a DFT bin at 250 Hz, so a tone there is where a
		// binning that only ever reached two of them would comb the worst.
		let spectrum = analyze(&[(250.0, 1.0)], 1196);
		let values = spectrum.values();

		let reached = |value: &f64| *value > 0.0;
		let first = values.iter().position(reached).expect("the tone is binned");
		let last = values
			.iter()
			.rposition(reached)
			.expect("the tone is binned");

		let gaps = values[first..=last].iter().filter(|&&v| v == 0.0).count();
		assert_eq!(
			gaps, 0,
			"a 250 Hz sine left {gaps} empty bins between bin {first} and bin {last}",
		);
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
		// The bins hold amplitude, so their power is the sum of their squares.
		let binned_power = spectrum
			.values()
			.iter()
			.map(|value| value * value)
			.sum::<f64>();

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

		// The bins hold amplitude; the split is of power.
		let power: Vec<f64> = spectrum
			.values()
			.iter()
			.map(|value| value * value)
			.collect();
		let lower_share = power[0] / (power[0] + power[1]);
		assert!(
			(lower_share - 0.75).abs() < 0.05,
			"the nearer bin should take three quarters of the power, took {lower_share}",
		);
		assert!(
			power[2] < 1e-9,
			"no power belongs two octaves above the tone",
		);
	}

	#[test]
	fn dc_has_no_log_frequency_and_reaches_no_bin() {
		let spectrum = generator(&[1.0; WINDOW]).generate(SpectrumBuffer::new(spectrum_params(64)));

		// A constant signal carries its power at DC, which the binning loop
		// skips because zero has no logarithm. What is left is the window's own
		// numerical leakage, orders of magnitude below the signal.
		let binned_power = spectrum
			.values()
			.iter()
			.map(|value| value * value)
			.sum::<f64>();
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

	#[test]
	fn the_update_rate_sets_the_tick_interval_whatever_the_window() {
		for window_ms in [10, 50, 1000] {
			for update_rate in [1, 25, 100] {
				let mut generator = generator(&[]);
				generator.set_config(Config {
					window_ms,
					update_rate,
				});

				let second = generator.interval().as_secs_f64() * update_rate as f64;
				// A `Duration` holds whole nanoseconds, so an interval that
				// divides a second unevenly does not multiply back to it exactly.
				assert!(
					(second - 1.0).abs() < 1e-6,
					"{update_rate} ticks at a {window_ms} ms window took {second} s",
				);
			}
		}
	}

	#[test]
	fn the_window_keeps_its_milliseconds_across_sample_rates() {
		let mut generator = generator(&[]);
		assert_eq!(generator.analyzer.window_size(), WINDOW);

		for (sample_rate, samples) in [(44_100, 2205), (96_000, 4800), (192_000, 9600)] {
			generator.set_sample_rate(sample_rate);
			assert_eq!(
				generator.analyzer.window_size(),
				samples,
				"{WINDOW_MS} ms at {sample_rate} Hz",
			);
		}
	}

	#[test]
	fn changing_the_sample_rate_updates_frequency_binning() {
		let window = window_samples(WINDOW_MS, 96_000);
		let signal = sine_wave(&[(440.0, 1.0)], 96_000, window);
		let mut generator = generator(&signal);
		generator.set_sample_rate(96_000);

		let spectrum = generator.generate(SpectrumBuffer::new(spectrum_params(1196)));
		let peak = spectrum.params().frequencies()[peak_index(&spectrum)];
		let dft_bin_hz = 96_000.0 / window as f64;

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
