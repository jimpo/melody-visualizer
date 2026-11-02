pub mod audio;
pub mod midi;

use std::{
	fmt::{self, Debug},
	sync::{
		Arc,
		atomic::{AtomicBool, Ordering},
	},
	time::Duration,
};

use crate::app::config::SpectrumGeneratorConfig;
use crate::audio::SampleReader;
use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumGenerator};
use audio::AudioSpectrumGenerator;
use midi::{NoteReader, Voices};

/// The generator the engine runs: the audio, until the host plays a note, and
/// the notes from then on.
///
/// The switch is one way. The notes are drained every tick whichever source is
/// drawn, so the ring never fills with stale events.
pub struct SourceGenerator {
	audio: AudioSpectrumGenerator,
	voices: Voices,
	/// Raised the tick the first note is played, for the editor to read.
	midi: Arc<AtomicBool>,
}

impl Debug for SourceGenerator {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		f.debug_struct("SourceGenerator")
			.field("audio", &self.audio)
			.field("midi", &self.voices.heard())
			.finish()
	}
}

impl SourceGenerator {
	pub fn new(
		config: SpectrumGeneratorConfig,
		samples: SampleReader,
		notes: NoteReader,
		sample_rate: u32,
		midi: Arc<AtomicBool>,
	) -> Self {
		SourceGenerator {
			audio: AudioSpectrumGenerator::new(config.audio, samples, sample_rate),
			voices: Voices::new(config.envelope, notes),
			midi,
		}
	}

	pub fn set_config(&mut self, config: SpectrumGeneratorConfig) {
		self.audio.set_config(config.audio);
		self.voices.envelope = config.envelope;
	}
}

impl SpectrumGenerator for SourceGenerator {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		// ponytail: ages voices by the configured tick, not the time elapsed, so
		// a tick skipped for want of a buffer stretches the envelopes slightly.
		self.voices.advance(self.interval().as_secs_f64());
		if !self.voices.heard() {
			return self.audio.generate(buffer);
		}
		self.midi.store(true, Ordering::Relaxed);
		buffer.fill(|values, params| self.voices.fill(values, params))
	}

	fn interval(&self) -> Duration {
		self.audio.interval()
	}

	fn set_sample_rate(&mut self, sample_rate: u32) {
		self.audio.set_sample_rate(sample_rate);
	}
}
