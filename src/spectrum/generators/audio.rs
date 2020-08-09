use jack::{Frames, RingBufferReader};
use itertools::Itertools;
use rustfft::{num_complex::Complex64, num_traits::Zero, FFTplanner, FFT};
use std::{
	cmp::Ordering,
	f64::consts::PI,
	fmt::{self, Debug},
	sync::Arc,
	time::Duration,
};

use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumGenerator};

// Half of the DFT window should overlap with the previous.
const TARGET_OVERLAP: (u64, u64) = (1, 2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
	pub dft_window_size: usize,
}

pub struct AudioSpectrumGenerator {
	audio_buffer: RingBufferReader,
	analyzer: Analyzer,
}

impl Debug for AudioSpectrumGenerator {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("AudioSpectrumGenerator")
			.field("analyzer", &self.analyzer)
			.finish()
	}
}

impl AudioSpectrumGenerator {
	pub fn new(config: Config, audio_buffer: RingBufferReader, sample_rate: Frames) -> Self {
		let mut generator = AudioSpectrumGenerator {
			audio_buffer,
			analyzer: Analyzer::new(WindowShape::Hann, sample_rate),
		};
		generator.set_window_size(config.dft_window_size);
		generator
	}

	pub fn set_window_size(&mut self, dft_window_size: usize) {
		self.analyzer.set_window_size(dft_window_size);
	}
}

impl SpectrumGenerator for AudioSpectrumGenerator {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		// Nothing to do if buffer has no samples.
		if buffer.params().samples() == 0 {
			return buffer.fill(|_, _| ());
		}

		let n = self.analyzer.window_size();

		let available = self.audio_buffer.space();
		if available > n * 4 {
			self.audio_buffer.advance(available - n * 4);
		}

		let samples = self.audio_buffer
			.peek_iter()
			.take(n * 4)
			.tuples::<(_, _, _, _)>()
			.map(|(&b1, &b2, &b3, &b4)| f32::from_ne_bytes([b1, b2, b3, b4]) as f64);

		self.analyzer.fill_spectrum(buffer, samples)
	}

	fn interval(&self) -> Duration {
		let (overlap_numerator, overlap_denominator) = TARGET_OVERLAP;
		Duration::from_micros(
			(1_000_000 * self.analyzer.window_size() as u64 * overlap_numerator) /
				(self.analyzer.sample_rate as u64 * overlap_denominator)
		)
	}
}

#[derive(Debug)]
enum WindowShape {
	Rectangular,
	Hann,
}

impl WindowShape {
	pub fn generate(&self, xs: &mut [f64]) {
		let size = xs.len();
		match self {
			Self::Rectangular => {
				for i in 0..size {
					xs[i] = 1.0;
				}
			}
			Self::Hann => {
				for i in 0..size {
					xs[i] = (PI * i as f64 / (size - 1) as f64).sin().powi(2);
				}
			}
		}
	}
}

struct Analyzer {
	window_shape: WindowShape,
	sample_rate: Frames,
	dft: Arc<dyn FFT<f64>>,
	dft_window: Vec<Complex64>,
	dft_output: Vec<Complex64>,
	windowing: Vec<f64>,
}

impl Analyzer {
	pub fn new(window_shape: WindowShape, sample_rate: Frames) -> Self {
		Analyzer {
			window_shape,
			sample_rate,
			dft: FFTplanner::new(false).plan_fft(0),
			dft_window: Vec::new(),
			dft_output: Vec::new(),
			windowing: Vec::new(),
		}
	}

	pub fn window_size(&self) -> usize {
		self.dft_window.len()
	}

	pub fn set_window_size(&mut self, dft_window_size: usize) {
		self.dft = FFTplanner::new(false).plan_fft(dft_window_size);
		self.dft_window = vec![Complex64::zero(); dft_window_size];
		self.dft_output = vec![Complex64::zero(); dft_window_size];
		self.windowing = vec![0.0; dft_window_size];
		self.window_shape.generate(&mut self.windowing);
	}

	// TODO: Test that this preserves square sum regardless of window size.
	fn fill_spectrum(&mut self, buffer: SpectrumBuffer, samples: impl Iterator<Item=f64>)
					 -> Spectrum
	{
		let n = self.dft_window.len();

		let windowed_samples = samples
			.zip(self.windowing.iter())
			.map(|(a, &b)| a * b);

		for (sample, dst) in windowed_samples.zip(self.dft_window.iter_mut()) {
			*dst = Complex64::new(sample, 0.0);
		}

		self.dft.process(&mut self.dft_window, &mut self.dft_output);

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

				let dft_out_val = dft_out_to_val(&self.dft_output[j], n);
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

	use rand::{rngs::StdRng, SeedableRng, RngCore};

	use crate::spectrum::SpectrumParams;

	#[test]
	fn fft_preserves_power() {
		let n = 2048;

		let mut input = vec![0.0; n];
		let mut output = vec![Complex64::zero(); n];

		let mut rng = StdRng::seed_from_u64(0);
		for x in input.iter_mut() {
			*x = rng.next_u32() as f64;
		}

		let dft = FFTplanner::new(false).plan_fft(n);
		let mut dft_input = input.iter()
			.map(|&val| Complex64::new(val, 0.0))
			.collect::<Vec<_>>();
		dft.process(&mut dft_input, &mut output);

		let input_power = input.iter().map(|x| x * x).sum::<f64>() / n as f64;
		let output_power = output.iter()
			.map(|x| dft_out_to_val(x, n))
			.sum::<f64>();
		assert!(input_power / output_power > 0.99999 && input_power / output_power < 1.00001);
	}
}