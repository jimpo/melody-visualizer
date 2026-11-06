//! The audio source as a constant-Q transform.
//!
//! A CQT bin is a window of `Q` periods of its own frequency, so every bin has
//! the same resolution relative to its pitch: a bass note is as sharp, in
//! semitones, as a treble one. Done directly that is O(bins × window) per tick.
//! Done as Brown and Puckette did (1992), it is one FFT plus a sparse dot
//! product per bin: each bin's windowed exponential is a narrow peak in the
//! frequency domain, so the bin is a handful of FFT outputs weighed by a
//! precomputed kernel.
//!
//! The bins are the display grid itself, so the output needs no re-binning.

use realfft::{RealFftPlanner, RealToComplex, num_complex::Complex64};
use std::{
	f64::consts::TAU,
	fmt::{self, Debug},
	sync::Arc,
	time::Duration,
};

use super::audio::{Config, window_samples};
use crate::audio::SampleReader;
use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumGenerator, SpectrumParams};

/// Periods of a bin's own frequency its window spans. At 24 the Hann main lobe
/// is about a semitone either side, so neighbouring semitones resolve.
const Q: f64 = 24.0;

/// How far a kernel reaches from its centre, in main-lobe half-widths (`N / L`
/// FFT bins each). A Hann sidelobe is below -60 dB by about four.
const KERNEL_REACH: f64 = 4.0;

/// The sparse spectral kernels for one frequency grid, sample rate and window.
struct Kernels {
	/// What these were built for; a mismatch means rebuild.
	params: Arc<SpectrumParams>,
	sample_rate: u32,
	window: usize,
	/// The first FFT output bin of each kernel.
	start: Vec<usize>,
	/// `weights[offsets[k]..offsets[k + 1]]` is bin `k`'s kernel.
	offsets: Vec<usize>,
	weights: Vec<Complex64>,
}

impl Kernels {
	/// Kernels over an FFT of `fft_size` samples whose newest `window` are the
	/// audio.
	///
	/// Bin `k` looks at the newest `L = min(Q·fs/f, window)` samples. The cap is
	/// what makes the window setting the longest time the analysis looks back:
	/// bins too low for `Q` periods to fit keep a window of `window` and a lower
	/// `Q`.
	///
	/// shortcut: the mirror image at negative frequency is ignored, so a kernel
	/// near DC (short window, low bin) is slightly off; upgrade by adding the
	/// image's weights if windows that short are ever offered with bass bins.
	fn build(params: &Arc<SpectrumParams>, sample_rate: u32, window: usize) -> Self {
		let fft_size = window.next_power_of_two();
		let fs = sample_rate as f64;
		let nyquist_bin = fft_size / 2;

		let mut kernels = Kernels {
			params: params.clone(),
			sample_rate,
			window,
			start: Vec::with_capacity(params.samples()),
			offsets: vec![0],
			weights: Vec::new(),
		};
		for &frequency in params.frequencies() {
			let length = ((Q * fs / frequency).round() as usize).clamp(3, window.max(3));
			// FFT bins per radian-per-sample of detuning, and the half-width of
			// the Hann main lobe in FFT bins.
			let centre = frequency / fs * fft_size as f64;
			let reach = KERNEL_REACH * 2.0 * fft_size as f64 / length as f64;
			let first = (centre - reach).floor().max(0.0) as usize;
			let last = ((centre + reach).ceil() as usize).min(nyquist_bin);

			kernels.start.push(first);
			if first <= last && frequency < fs / 2.0 {
				let theta = TAU * frequency / fs;
				// The kernel is right-aligned: it covers the last `length`
				// samples of the FFT frame, which is a linear phase here.
				let shift = (fft_size - length) as f64;
				// Divided by the window's sum, so a sine of amplitude A scores
				// A/2; doubled, because only the positive frequency is summed.
				let scale = 2.0 / ((length - 1) as f64 / 2.0) / fft_size as f64;
				for m in first..=last {
					let omega = TAU * m as f64 / fft_size as f64;
					let phase = Complex64::from_polar(1.0, omega * shift);
					kernels
						.weights
						.push(phase * hann_transform(omega - theta, length) * scale);
				}
			}
			kernels.offsets.push(kernels.weights.len());
		}
		kernels
	}
}

/// `Σ_{u<length} hann(u) · e^{jφu}` for the symmetric Hann window
/// `sin²(πu / (length - 1))`, in closed form: three shifted Dirichlet kernels.
fn hann_transform(phi: f64, length: usize) -> Complex64 {
	let n = length as f64;
	let alpha = TAU / (n - 1.0);
	let dirichlet = |x: f64| -> Complex64 {
		let denominator = (x / 2.0).sin();
		if denominator.abs() < 1e-9 {
			// A multiple of 2π: every term is 1.
			return Complex64::new(n, 0.0);
		}
		Complex64::from_polar((n * x / 2.0).sin() / denominator, x * (n - 1.0) / 2.0)
	};
	0.5 * dirichlet(phi) - 0.25 * dirichlet(phi + alpha) - 0.25 * dirichlet(phi - alpha)
}

pub struct CqtSpectrumGenerator {
	audio_buffer: SampleReader,
	/// The newest window, read from the ring. Held across ticks so the spectrum
	/// thread allocates nothing per tick.
	samples: Vec<f32>,
	sample_rate: u32,
	config: Config,
	fft: Arc<dyn RealToComplex<f64>>,
	fft_in: Vec<f64>,
	fft_out: Vec<Complex64>,
	fft_scratch: Vec<Complex64>,
	kernels: Option<Kernels>,
	/// The overrun count as of the last tick that logged one.
	reported_overruns: u64,
}

impl Debug for CqtSpectrumGenerator {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("CqtSpectrumGenerator")
			.field("sample_rate", &self.sample_rate)
			.field("config", &self.config)
			.finish()
	}
}

impl CqtSpectrumGenerator {
	pub fn new(config: Config, audio_buffer: SampleReader, sample_rate: u32) -> Self {
		let fft = RealFftPlanner::new().plan_fft_forward(1);
		let mut generator = CqtSpectrumGenerator {
			audio_buffer,
			samples: Vec::new(),
			sample_rate,
			config,
			fft_in: fft.make_input_vec(),
			fft_out: fft.make_output_vec(),
			fft_scratch: fft.make_scratch_vec(),
			fft,
			kernels: None,
			reported_overruns: 0,
		};
		generator.plan_window();
		generator
	}

	pub fn set_config(&mut self, config: Config) {
		self.config = config;
		self.plan_window();
	}

	fn window(&self) -> usize {
		window_samples(self.config.window_ms, self.sample_rate)
	}

	/// Sizes the FFT to the window, re-planning only when the size changes.
	/// The kernels follow lazily in `generate`.
	fn plan_window(&mut self) {
		let window = self.window();
		let size = if window == 0 {
			0
		} else {
			window.next_power_of_two()
		};
		if size != self.fft_in.len() {
			self.fft = RealFftPlanner::new().plan_fft_forward(size.max(1));
			self.fft_in = self.fft.make_input_vec();
			self.fft_out = self.fft.make_output_vec();
			self.fft_scratch = self.fft.make_scratch_vec();
		}
		self.samples.resize(window, 0.0);
	}

	/// Log any audio the real-time thread dropped since the last tick.
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

impl SpectrumGenerator for CqtSpectrumGenerator {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		let window = self.window();
		// Nothing to analyze: no bins to fill, or a rate that gives no window.
		if buffer.params().samples() == 0 || window < 3 || self.sample_rate == 0 {
			return buffer.fill(|values, _| values.fill(0.0));
		}

		self.report_overruns();

		let stale = self.kernels.as_ref().is_none_or(|kernels| {
			!Arc::ptr_eq(&kernels.params, buffer.params())
				|| kernels.sample_rate != self.sample_rate
				|| kernels.window != window
		});
		if stale {
			self.kernels = Some(Kernels::build(buffer.params(), self.sample_rate, window));
		}
		let kernels = self.kernels.as_ref().expect("built above");

		// Newest sample last: a short read (the ring refilling after the window
		// grew) leaves silence before the audio, where the past would be.
		let read = self.audio_buffer.read_latest(&mut self.samples);
		let split = self.fft_in.len() - read;
		let (silence, audio) = self.fft_in.split_at_mut(split);
		silence.fill(0.0);
		for (dst, &sample) in audio.iter_mut().zip(&self.samples[..read]) {
			*dst = sample as f64;
		}
		self.fft
			.process_with_scratch(&mut self.fft_in, &mut self.fft_out, &mut self.fft_scratch)
			.expect("buffers are sized by plan_window");

		buffer.fill(|values, _| {
			for (k, value) in values.iter_mut().enumerate() {
				let weights = &kernels.weights[kernels.offsets[k]..kernels.offsets[k + 1]];
				let bins = &self.fft_out[kernels.start[k]..][..weights.len()];
				let sum: Complex64 = bins.iter().zip(weights).map(|(x, w)| x * w).sum();
				*value = sum.norm();
			}
		})
	}

	/// The time between two spectra: one over the update rate.
	fn interval(&self) -> Duration {
		Duration::from_secs_f64(1.0 / self.config.update_rate.max(1) as f64)
	}

	fn set_sample_rate(&mut self, sample_rate: u32) {
		self.sample_rate = sample_rate;
		self.plan_window();
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use crate::test_support::{sample_reader, sine_wave, spectrum_params};

	const SAMPLE_RATE: u32 = 48_000;
	const WINDOW_MS: u32 = 300;
	const WINDOW: usize = 14_400;

	fn generator(samples: &[f32], sample_rate: u32) -> CqtSpectrumGenerator {
		CqtSpectrumGenerator::new(
			Config {
				window_ms: WINDOW_MS,
				..Config::default()
			},
			sample_reader(samples),
			sample_rate,
		)
	}

	fn analyze(components: &[(f64, f64)]) -> Spectrum {
		let signal = sine_wave(components, SAMPLE_RATE, WINDOW);
		generator(&signal, SAMPLE_RATE).generate(SpectrumBuffer::new(spectrum_params(1196)))
	}

	fn peak(spectrum: &Spectrum) -> (f64, f64) {
		let (index, &value) = spectrum
			.values()
			.iter()
			.enumerate()
			.max_by(|(_, a), (_, b)| a.partial_cmp(b).expect("no NaN"))
			.expect("the spectrum has bins");
		(spectrum.params().frequencies()[index], value)
	}

	#[test]
	fn a_sine_peaks_at_its_frequency_with_its_amplitude_in_every_octave() {
		for frequency in [220.0, 440.0, 1760.0, 7040.0] {
			let (peak_frequency, amplitude) = peak(&analyze(&[(frequency, 0.5)]));

			assert!(
				(peak_frequency / frequency - 1.0).abs() < 0.01,
				"a {frequency} Hz sine peaked at {peak_frequency} Hz",
			);
			assert!(
				(amplitude / 0.5 - 1.0).abs() < 0.05,
				"a {frequency} Hz sine of amplitude 0.5 measured {amplitude}",
			);
		}
	}

	#[test]
	fn tones_three_semitones_apart_are_resolved() {
		let high = 440.0 * 2f64.powf(3.0 / 12.0);
		let spectrum = analyze(&[(440.0, 1.0), (high, 1.0)]);
		let value_at = |frequency: f64| {
			let (index, _) = spectrum
				.params()
				.frequencies()
				.iter()
				.enumerate()
				.min_by(|(_, a), (_, b)| {
					(*a - frequency)
						.abs()
						.partial_cmp(&(*b - frequency).abs())
						.unwrap()
				})
				.unwrap();
			spectrum.values()[index]
		};

		let between = value_at((440.0 * high).sqrt());
		assert!(
			between < 0.2 * value_at(440.0).min(value_at(high)),
			"the midpoint reads {between}, the tones {} and {}",
			value_at(440.0),
			value_at(high),
		);
	}

	#[test]
	fn silence_produces_an_empty_spectrum() {
		assert!(analyze(&[]).values().iter().all(|&value| value == 0.0));
	}

	#[test]
	fn a_spectrum_with_no_bins_is_returned_untouched() {
		let signal = sine_wave(&[(440.0, 1.0)], SAMPLE_RATE, WINDOW);

		let spectrum = generator(&signal, SAMPLE_RATE).generate(SpectrumBuffer::default());

		assert!(spectrum.values().is_empty());
	}

	#[test]
	fn a_short_read_is_padded_with_silence() {
		// A quarter of the window, as just after the slider moved up.
		let signal = sine_wave(&[(1760.0, 1.0)], SAMPLE_RATE, WINDOW / 4);
		let spectrum =
			generator(&signal, SAMPLE_RATE).generate(SpectrumBuffer::new(spectrum_params(1196)));

		let (frequency, amplitude) = peak(&spectrum);
		assert!((frequency / 1760.0 - 1.0).abs() < 0.01);
		assert!(amplitude.is_finite() && amplitude > 0.0 && amplitude <= 1.05);
	}

	#[test]
	fn changing_the_sample_rate_rebuilds_the_kernels() {
		let signal = sine_wave(&[(440.0, 1.0)], 96_000, window_samples(WINDOW_MS, 96_000));
		let mut generator = generator(&signal, SAMPLE_RATE);
		let params = spectrum_params(1196);
		generator.generate(SpectrumBuffer::new(params.clone()));
		generator.set_sample_rate(96_000);

		let (frequency, amplitude) = peak(&generator.generate(SpectrumBuffer::new(params)));

		assert!((frequency / 440.0 - 1.0).abs() < 0.01);
		assert!((amplitude - 1.0).abs() < 0.05);
	}

	#[test]
	fn the_update_rate_sets_the_tick_interval() {
		let mut generator = generator(&[], SAMPLE_RATE);
		for update_rate in [1, 25, 100] {
			generator.set_config(Config {
				update_rate,
				..Config::default()
			});
			let second = generator.interval().as_secs_f64() * update_rate as f64;
			assert!((second - 1.0).abs() < 1e-6);
		}
	}
}
