use jack::{Frames, RingBufferReader};
use log::debug;
use itertools::Itertools;
use rustfft::{num_complex::Complex64, num_traits::Zero, FFTplanner, FFT};
use std::{
	fmt::{self, Debug},
	sync::Arc,
	time::Duration,
};

use crate::spectrum_renderer::SpectrumGenerator;
use crate::spectrum::{SpectrumBuffer, Spectrum};

// Half of the DFT window should overlap with the previous.
const TARGET_OVERLAP: (u64, u64) = (1, 2);

pub struct AudioSpectrumGenerator {
	audio_buffer: RingBufferReader,
	sample_rate: Frames,
	dft: Arc<dyn FFT<f64>>,
	dft_window: Vec<Complex64>,
	dft_output: Vec<Complex64>,
}

impl Debug for AudioSpectrumGenerator {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("AudioSpectrumGenerator")
			.field("sample_rate", &self.sample_rate)
			.finish()
	}
}

impl AudioSpectrumGenerator {
	pub fn new(audio_buffer: RingBufferReader, sample_rate: Frames, dft_window_size: Frames)
		-> Self
	{
		let dft = FFTplanner::new(false).plan_fft(dft_window_size as usize);
		let dft_window = vec![Complex64::zero(); dft_window_size as usize];
		let dft_output = vec![Complex64::zero(); dft_window_size as usize];
		AudioSpectrumGenerator {
			audio_buffer,
			sample_rate,
			dft,
			dft_window,
			dft_output,
		}
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

		for (sample, dst) in samples.zip(self.dft_window.iter_mut()) {
			*dst = Complex64::new(sample, 0.0);
		}

		// TODO: Apply Hann window.

		self.dft.process(&mut self.dft_window, &mut self.dft_output);

		// TODO: Take norms and divide by N.

		buffer.fill(|spectrum, spectrum_params| {
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
				while dft_out_freq >= log_freqs[i] {
					i += 1;
					if i >= log_freqs.len() {
						return;
					}
				}

				let dft_out_val = self.dft_out_to_val(&self.dft_output[j]);
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

impl AudioSpectrumGenerator {
	fn dft_out_to_val(&self, dft_out: &Complex64) -> f64 {
		dft_out.norm()
	}
}