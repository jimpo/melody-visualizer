use nice_plug::prelude::*;
use nice_plug_iced::{IcedEditor, IcedEditorState, iced::PollSubNotifier};
use parking_lot::Mutex;
use std::iter;
use std::sync::Arc;

use audio::ring::SampleWriter;
use engine::Engine;
use params::PluginParams;
use spectrum::generators::midi::NoteWriter;

pub mod app;
pub mod async_processor;
pub mod audio;
mod editor;
pub mod engine;
pub mod error;
pub mod graphic;
pub mod note;
pub mod params;
pub mod spectrum;
#[cfg(any(test, feature = "testing"))]
pub mod test_support;
pub mod traits;

/// The plugin. It passes its input through untouched and analyzes a mono mix
/// of it, or, once the host plays it a note, draws the notes instead.
pub struct MelodyVisualizer {
	params: Arc<PluginParams>,
	editor_state: Arc<IcedEditorState>,
	/// Wakes the editor to draw a frame.
	notifier: PollSubNotifier,
	/// The engine while the plugin is active. The editor reconfigures it and
	/// asks it for frames; the audio thread never touches it.
	engine: Arc<Mutex<Option<Engine>>>,
	/// Where `process` writes the audio the engine analyzes, while the plugin
	/// is active.
	writer: Option<SampleWriter>,
	/// Where `process` writes the host's note events, while the plugin is
	/// active.
	notes: Option<NoteWriter>,
	/// Room for one block's mono mix, so `process` allocates nothing.
	mono: Vec<f32>,
}

impl Default for MelodyVisualizer {
	fn default() -> Self {
		// nice-plug logs through `tracing`, and the engine through `log`. This
		// sends the engine's records to the same place; it fails harmlessly
		// once another instance has installed it.
		let _ = tracing_log::LogTracer::init();
		Self {
			params: Arc::default(),
			editor_state: editor::default_state(),
			notifier: PollSubNotifier::new(),
			engine: Arc::default(),
			writer: None,
			notes: None,
			mono: Vec::new(),
		}
	}
}

impl Plugin for MelodyVisualizer {
	const NAME: &'static str = "Melody Visualizer";
	const VENDOR: &'static str = "Jim Posen";
	const URL: &'static str = "";
	const EMAIL: &'static str = "jim.posen@gmail.com";
	const VERSION: &'static str = env!("CARGO_PKG_VERSION");

	const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
		AudioIOLayout {
			main_input_channels: NonZeroU32::new(2),
			main_output_channels: NonZeroU32::new(2),
			..AudioIOLayout::const_default()
		},
		AudioIOLayout {
			main_input_channels: NonZeroU32::new(1),
			main_output_channels: NonZeroU32::new(1),
			..AudioIOLayout::const_default()
		},
	];

	const MIDI_INPUT: MidiConfig = MidiConfig::Basic;

	type Editor = IcedEditor;
	type SysExMessage = ();
	type BackgroundTask = ();

	fn params(&self) -> Arc<dyn Params> {
		self.params.clone()
	}

	fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Self::Editor> {
		editor::create(
			self.editor_state.clone(),
			self.params.clone(),
			self.engine.clone(),
			self.notifier.clone(),
		)
	}

	fn activate(
		&mut self,
		_audio_io_layout: &AudioIOLayout,
		buffer_config: &BufferConfig,
		_context: &mut impl ActivateContext<Self>,
	) -> bool {
		// The host may activate again without deactivating first, for instance
		// to restore state, so stop the old engine before starting the new one.
		self.deactivate();
		let sample_rate = buffer_config.sample_rate.round() as u32;
		match Engine::start(&self.params.config(), sample_rate) {
			Ok((engine, writer, notes)) => {
				*self.engine.lock() = Some(engine);
				self.writer = Some(writer);
				self.notes = Some(notes);
			}
			Err(err) => {
				log::error!("failed to start the engine: {err}");
				return false;
			}
		}
		self.mono = vec![0.0; buffer_config.max_buffer_size as usize];
		true
	}

	fn deactivate(&mut self) {
		*self.engine.lock() = None;
		self.writer = None;
		self.notes = None;
	}

	fn process(
		&mut self,
		buffer: &mut Buffer,
		_aux: &mut AuxiliaryBuffers,
		context: &mut impl ProcessContext<Self>,
	) -> ProcessStatus {
		while let Some(event) = context.next_event() {
			if let Some(notes) = &mut self.notes {
				// ponytail: an event that finds the ring full is dropped
				// unannounced, and a dropped note-off leaves its note on. The
				// ring holds a thousand events a tick; count drops as the
				// sample ring does if that is ever reached.
				let _ = notes.push(event);
			}
		}
		// The editor draws a frame each time it is woken, so new audio is what
		// paces the frames.
		if self.editor_state.is_open() {
			self.notifier.notify();
		}
		let Some(writer) = &mut self.writer else {
			return ProcessStatus::Normal;
		};
		let samples = buffer.samples().min(self.mono.len());
		match buffer.as_slice_immutable() {
			[channel] => writer.write_samples(channel),
			channels => {
				let mono = &mut self.mono[..samples];
				mono.fill(0.0);
				let gain = 1.0 / channels.len() as f32;
				for channel in channels {
					for (mixed, sample) in iter::zip(mono.iter_mut(), channel.iter()) {
						*mixed += sample * gain;
					}
				}
				writer.write_samples(mono);
			}
		}
		ProcessStatus::Normal
	}
}

impl ClapPlugin for MelodyVisualizer {
	// The host saves this in every project that uses the plugin: never change it.
	const CLAP_ID: &'static str = "info.jimpo.melody-visualizer";
	const CLAP_DESCRIPTION: Option<&'static str> =
		Some("Draws the pitches it hears on a spiral, one turn per octave");
	const CLAP_MANUAL_URL: Option<&'static str> = None;
	const CLAP_SUPPORT_URL: Option<&'static str> = None;
	const CLAP_FEATURES: &'static [ClapFeature] = &[
		ClapFeature::AudioEffect,
		ClapFeature::Analyzer,
		ClapFeature::Stereo,
		ClapFeature::Mono,
	];
}

nice_export_clap!(MelodyVisualizer);
