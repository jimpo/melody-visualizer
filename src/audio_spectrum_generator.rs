use jack::{Frames, RingBufferReader};
use log::debug;
use std::fmt::{self, Debug};
use std::time::Duration;

use crate::spectrum_renderer::SpectrumGenerator;
use crate::spectrum::{SpectrumBuffer, Spectrum};

pub struct AudioSpectrumGenerator {
	audio_buffer: RingBufferReader,
	sample_rate: Frames,
	dft_window_size: Frames,
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
		AudioSpectrumGenerator {
			audio_buffer,
			sample_rate,
			dft_window_size,
		}
	}
}

impl SpectrumGenerator for AudioSpectrumGenerator {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		debug!("generating spectrum");
		buffer.fill(|_, _| ())
	}

	fn interval(&self) -> Duration {
		Duration::from_micros(1_000_000 * self.dft_window_size as u64 / self.sample_rate as u64)
	}
}