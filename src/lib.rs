use nice_plug::prelude::*;
use nice_plug_iced::{IcedEditor, IcedEditorState};
use std::sync::Arc;

mod editor;

/// The plugin. For now it passes its input through untouched.
pub struct MelodyVisualizer {
	params: Arc<MelodyVisualizerParams>,
	editor_state: Arc<IcedEditorState>,
}

impl Default for MelodyVisualizer {
	fn default() -> Self {
		Self {
			params: Arc::default(),
			editor_state: editor::default_state(),
		}
	}
}

#[derive(Params, Default)]
struct MelodyVisualizerParams {}

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

	type Editor = IcedEditor;
	type SysExMessage = ();
	type BackgroundTask = ();

	fn params(&self) -> Arc<dyn Params> {
		self.params.clone()
	}

	fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Self::Editor> {
		editor::create(self.editor_state.clone())
	}

	fn process(
		&mut self,
		_buffer: &mut Buffer,
		_aux: &mut AuxiliaryBuffers,
		_context: &mut impl ProcessContext<Self>,
	) -> ProcessStatus {
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
