//! The spectrum and graphic threads, started from a [`Config`] for one sample
//! rate.

use futures::{channel::mpsc, executor::block_on};

use crate::app::Config;
use crate::app::config::SpectrumGeneratorConfig;
use crate::async_processor::AsyncProcessor;
use crate::audio::ring::{SampleWriter, sample_ring};
use crate::error::Error;
use crate::graphic::renderer::{self as graphic_renderer, GraphicRenderer};
use crate::spectrum::generators::audio::{self, AudioSpectrumGenerator};
use crate::spectrum::renderer::{self as spectrum_renderer, SpectrumRenderer};

/// The running engine. Dropping it stops both threads.
pub struct Engine {
	_spectrum: AsyncProcessor<SpectrumRenderer>,
	_graphic: AsyncProcessor<GraphicRenderer>,
}

impl Engine {
	/// Starts both threads configured as `config` describes, analyzing audio at
	/// `sample_rate`, and returns the engine with the writer the audio goes in
	/// through.
	///
	/// Blocks until both threads have taken their configuration.
	pub fn start(config: &Config, sample_rate: u32) -> Result<(Self, SampleWriter), Error> {
		// Spectra flow from the spectrum thread to the graphic thread, and their
		// emptied buffers flow back.
		let (spectrum_graphic_tx, spectrum_graphic_rx) = mpsc::channel(0);
		let (graphic_spectrum_tx, graphic_spectrum_rx) = mpsc::channel(0);
		let mut graphic = graphic_renderer::start(spectrum_graphic_rx, graphic_spectrum_tx)?;
		let mut spectrum = spectrum_renderer::start(graphic_spectrum_rx, spectrum_graphic_tx)?;

		let (writer, reader) = sample_ring(ring_capacity(sample_rate));
		let SpectrumGeneratorConfig::Audio(generator_config) = config.spectrum_generator.clone();
		let transforms = config.spectrum_transforms.clone();
		block_on(spectrum.exec(move |renderer| {
			renderer.set_generator(Box::new(AudioSpectrumGenerator::new(
				generator_config,
				reader,
				sample_rate,
			)));
			*renderer.transforms_mut() = transforms.into_iter().collect();
		}))?;

		let spectrum_params = config.spectrum_params();
		let generator_config = config.graphic_generator.clone();
		block_on(graphic.exec(move |renderer| {
			renderer.set_spectrum_params(spectrum_params);
			renderer.update_generator(|generator| generator_config.update(generator));
		}))?;

		Ok((
			Engine {
				_spectrum: spectrum,
				_graphic: graphic,
			},
			writer,
		))
	}
}

/// The capture ring's size in samples: the longest window plus the longest hop,
/// so the audio between two ticks never overruns it.
fn ring_capacity(sample_rate: u32) -> usize {
	let milliseconds = audio::MAX_WINDOW_MS + 1000 / audio::MIN_UPDATE_RATE;
	(milliseconds as usize * sample_rate as usize).div_ceil(1000)
}
