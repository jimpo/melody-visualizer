use jack::{Frames, RingBufferReader};
use std::fmt::{self, Debug};

use crate::spectrum_renderer::SpectrumGenerator;
use crate::spectrum::{SpectrumBuffer, Spectrum};

pub struct AudioSpectrumGenerator {
	audio_buffer: RingBufferReader,
	sample_rate: Frames,
}

impl Debug for AudioSpectrumGenerator {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("AudioSpectrumGenerator")
			.field("sample_rate", &self.sample_rate)
			.finish()
	}
}

impl AudioSpectrumGenerator {
	pub fn new(audio_buffer: RingBufferReader, sample_rate: Frames) -> Self {
		AudioSpectrumGenerator {
			audio_buffer,
			sample_rate,
		}
	}
}

impl SpectrumGenerator for AudioSpectrumGenerator {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		unimplemented!()
	}
}