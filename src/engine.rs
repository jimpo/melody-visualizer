//! The spectrum and graphic threads, started from a [`Config`] for one sample
//! rate.

use futures::{channel::mpsc, executor::block_on};
use std::iter;
use std::sync::{
	Arc,
	atomic::{AtomicBool, Ordering},
};

use crate::app::Config;
use crate::async_processor::AsyncProcessor;
use crate::audio::ring::{SampleWriter, sample_ring};
use crate::error::Error;
use crate::graphic::renderer::{self as graphic_renderer, GraphicRenderer};
use crate::graphic::{Graphic, GraphicBuffer};
use crate::spectrum::generators::{
	SourceGenerator, audio,
	midi::{self, NoteWriter},
};
use crate::spectrum::renderer::{self as spectrum_renderer, SpectrumRenderer};

/// The running engine. Dropping it stops both threads.
///
/// Every call blocks until the thread it addresses has answered. Each thread
/// answers between two frames or two ticks, well under a millisecond, so the
/// caller can be the editor's own thread.
pub struct Engine {
	spectrum: AsyncProcessor<SpectrumRenderer>,
	graphic: AsyncProcessor<GraphicRenderer>,
	/// The configuration the threads run, as last applied.
	applied: Config,
	/// Whether the generator has switched to drawing MIDI notes.
	midi: Arc<AtomicBool>,
}

impl Engine {
	/// Starts both threads configured as `config` describes, analyzing audio at
	/// `sample_rate`, and returns the engine with the writers the audio and the
	/// note events go in through.
	pub fn start(
		config: &Config,
		sample_rate: u32,
	) -> Result<(Self, SampleWriter, NoteWriter), Error> {
		// Spectra flow from the spectrum thread to the graphic thread, and their
		// emptied buffers flow back.
		let (spectrum_graphic_tx, spectrum_graphic_rx) = mpsc::channel(0);
		let (graphic_spectrum_tx, graphic_spectrum_rx) = mpsc::channel(0);
		let mut graphic = graphic_renderer::start(spectrum_graphic_rx, graphic_spectrum_tx)?;
		let mut spectrum = spectrum_renderer::start(graphic_spectrum_rx, spectrum_graphic_tx)?;

		let (writer, reader) = sample_ring(ring_capacity(sample_rate));
		let (note_writer, note_reader) = rtrb::RingBuffer::new(midi::RING_CAPACITY);
		let midi = Arc::new(AtomicBool::new(false));
		let generator = SourceGenerator::new(
			config.spectrum_generator.clone(),
			reader,
			note_reader,
			sample_rate,
			midi.clone(),
		);
		let transforms = config.spectrum_transforms.clone();
		block_on(spectrum.exec(move |renderer| {
			renderer.set_generator(Box::new(generator));
			*renderer.transforms_mut() = transforms.into_iter().collect();
		}))?;

		let spectrum_params = config.spectrum_params();
		let generator_config = config.graphic_generator.clone();
		block_on(graphic.exec(move |renderer| {
			renderer.set_spectrum_params(spectrum_params);
			renderer.update_generator(|generator| generator_config.update(generator));
		}))?;

		let engine = Engine {
			spectrum,
			graphic,
			applied: config.clone(),
			midi,
		};
		Ok((engine, writer, note_writer))
	}

	/// Whether the engine draws the notes the host plays rather than its audio.
	/// It starts on audio and switches for good once a note is played.
	pub fn hears_midi(&self) -> bool {
		self.midi.load(Ordering::Relaxed)
	}

	/// Reconfigures the running threads to `config`, touching only what changed
	/// since the last call.
	///
	/// A transform is reconfigured in place rather than rebuilt, so a stage that
	/// keeps state across frames keeps it while one of its settings moves.
	///
	/// # Preconditions
	/// - `config` holds the same transforms, in the same order, as the config
	///   the engine was started with.
	pub fn apply(&mut self, config: &Config) -> Result<(), Error> {
		if *config == self.applied {
			return Ok(());
		}

		if config.spectrum_generator != self.applied.spectrum_generator {
			let generator_config = config.spectrum_generator.clone();
			block_on(
				self.spectrum
					.exec(move |renderer| generator_config.update(renderer.generator_mut())),
			)??;
		}

		for (entry, applied) in iter::zip(
			&config.spectrum_transforms,
			&self.applied.spectrum_transforms,
		) {
			if entry == applied {
				continue;
			}
			let entry = entry.clone();
			block_on(self.spectrum.exec(move |renderer| {
				let id = entry.id;
				let transforms = renderer.transforms_mut();
				transforms
					.set_enabled(id, entry.enabled)
					.ok_or(Error::MissingTransform { id })?;
				let transform = transforms
					.get_mut(id)
					.ok_or(Error::MissingTransform { id })?;
				entry.config.update(transform)
			}))??;
		}

		let spectrum_params = config.spectrum_params();
		if spectrum_params != self.applied.spectrum_params() {
			block_on(
				self.graphic
					.exec(move |renderer| renderer.set_spectrum_params(spectrum_params)),
			)?;
		}

		if config.graphic_generator != self.applied.graphic_generator {
			let generator_config = config.graphic_generator.clone();
			block_on(self.graphic.exec(move |renderer| {
				renderer.update_generator(|generator| generator_config.update(generator))
			}))?;
		}

		self.applied = config.clone();
		Ok(())
	}

	/// Draws the current spectrum history as a `width` by `height` frame.
	// ponytail: allocates a frame's pixels per call, since the editor hands
	// them to iced rather than back. Recycle them if the allocation shows up in
	// a profile.
	pub fn render(&mut self, width: u32, height: u32) -> Result<Graphic, Error> {
		let buffer = GraphicBuffer::new(width as i32, height as i32);
		block_on(self.graphic.exec(move |renderer| renderer.render(buffer)))?
	}
}

/// The capture ring's size in samples: the longest window plus the longest hop,
/// so the audio between two ticks never overruns it.
fn ring_capacity(sample_rate: u32) -> usize {
	let milliseconds = audio::MAX_WINDOW_MS + 1000 / audio::MIN_UPDATE_RATE;
	(milliseconds as usize * sample_rate as usize).div_ceil(1000)
}

#[cfg(test)]
mod tests {
	use super::*;

	use crate::app::config::GraphicGeneratorConfig;

	#[test]
	fn an_applied_change_reaches_the_frames_the_engine_draws() {
		let config = Config::default();
		let (mut engine, _writer, _notes) = Engine::start(&config, 48_000).unwrap();
		let before = engine.render(400, 300).unwrap().into_rgba();
		assert_eq!((before.0, before.1), (400, 300));

		// A key a fifth up turns every hue on the wheel.
		let mut changed = config.clone();
		let GraphicGeneratorConfig::Spiral(spiral) = &mut changed.graphic_generator;
		spiral.key_log_freq += 7.0 / 12.0;
		engine.apply(&changed).unwrap();

		let after = engine.render(400, 300).unwrap().into_rgba();
		assert!(after != before, "the frame drawn after the change differs");
	}

	#[test]
	fn a_note_event_switches_the_engine_to_midi() {
		use nice_plug::midi::{Channel, Key, NoteEvent, VoiceID};
		use std::time::{Duration, Instant};

		let (engine, _writer, mut notes) = Engine::start(&Config::default(), 48_000).unwrap();
		assert!(!engine.hears_midi(), "it starts on audio");

		notes
			.push(NoteEvent::NoteOn {
				timing: 0,
				voice_id: VoiceID::Wildcard,
				channel: Channel::Number(0),
				key: Key::Number(69),
				velocity: 1.0,
			})
			.unwrap();
		let deadline = Instant::now() + Duration::from_secs(2);
		while !engine.hears_midi() {
			assert!(Instant::now() < deadline, "the next tick switches it");
			std::thread::sleep(Duration::from_millis(5));
		}
	}
}
