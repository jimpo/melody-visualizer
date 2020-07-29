use jack::{Frames, RingBufferReader};
use log::debug;
use itertools::Itertools;
use rustfft::{num_complex::Complex64, num_traits::Zero, FFTplanner, FFT};
use std::{
	f64::consts::PI,
	fmt::{self, Debug},
	sync::Arc,
	time::Duration,
};

use crate::spectrum_renderer::SpectrumGenerator;
use crate::spectrum::{SpectrumBuffer, Spectrum};

// Half of the DFT window should overlap with the previous.
const TARGET_OVERLAP: (u64, u64) = (1, 2);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
	pub dft_window_size: usize,
}

pub struct AudioSpectrumGenerator {
	audio_buffer: RingBufferReader,
	sample_rate: Frames,
	dft: Arc<dyn FFT<f64>>,
	dft_window: Vec<Complex64>,
	dft_output: Vec<Complex64>,
	windowing: Vec<f64>,
}

impl Debug for AudioSpectrumGenerator {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("AudioSpectrumGenerator")
			.field("sample_rate", &self.sample_rate)
			.finish()
	}
}

impl AudioSpectrumGenerator {
	pub fn new(config: Config, audio_buffer: RingBufferReader, sample_rate: Frames) -> Self {
		let mut generator = AudioSpectrumGenerator {
			audio_buffer,
			sample_rate,
			dft: FFTplanner::new(false).plan_fft(0),
			dft_window: Vec::new(),
			dft_output: Vec::new(),
			windowing: Vec::new(),
		};
		generator.set_window_size(config.dft_window_size);
		generator
	}

	pub fn set_window_size(&mut self, dft_window_size: usize) {
		self.dft = FFTplanner::new(false).plan_fft(dft_window_size);
		self.dft_window = vec![Complex64::zero(); dft_window_size];
		self.dft_output = vec![Complex64::zero(); dft_window_size];
		self.windowing = hann_window(dft_window_size);
	}

	fn dft_out_to_val(&self, dft_out: &Complex64, n: usize) -> f64 {
		(dft_out / n as f64).norm()
	}
}

impl SpectrumGenerator for AudioSpectrumGenerator {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		// Nothing to do if buffer has no samples.
		if buffer.params().samples() == 0 {
			return buffer.fill(|_, _| ());
		}

		let n = self.dft_window.len();

		let available = self.audio_buffer.space();
		if available > n * 4 {
			self.audio_buffer.advance(available - n * 4);
		}

		let samples = self.audio_buffer
			.peek_iter()
			.take(n * 4)
			.tuples::<(_, _, _, _)>()
			.map(|(&b1, &b2, &b3, &b4)| f32::from_ne_bytes([b1, b2, b3, b4]) as f64);

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
			// are complex conjugates of samples 1, .., n / 2. Looked at another way, the DFT
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

				let dft_out_val = self.dft_out_to_val(&self.dft_output[j], n);
				let interp_ratio =
					(dft_out_log_freq - log_freqs[i - 1]) / (log_freqs[i] - log_freqs[i - 1]);
				spectrum[i - 1] += (1.0 - interp_ratio) * dft_out_val;
				spectrum[i] += interp_ratio * dft_out_val;
			}
		})
	}

	fn interval(&self) -> Duration {
		let (overlap_numerator, overlap_denominator) = TARGET_OVERLAP;
		Duration::from_micros(
			(1_000_000 * self.dft_window.len() as u64 * overlap_numerator) /
				(self.sample_rate as u64 * overlap_denominator)
		)
	}
}

fn hann_window(size: usize) -> Vec<f64> {
	let mut window = vec![0.0; size];
	for i in 0..size {
		window[i] = (PI * i as f64 / (size - 1) as f64).sin().powi(2);
	}
	window
}
