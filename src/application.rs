use futures::{prelude::*, channel::mpsc};
use glib::MainContext;
use log::{debug, error};
use std::{
	any::Any,
	cell::{RefCell, RefMut},
	mem,
	rc::Rc,
};

use crate::audio::AudioSourceController;
use crate::audio_spectrum_generator::AudioSpectrumGenerator;
use crate::async_processor::AsyncProcessor;
use crate::error::Error;
use crate::graphic::{Graphic, GraphicBuffer};
use crate::graphic_renderer::{self, GraphicRenderer};
use crate::pubsub::{Notifier, PubSub};
use crate::source::{JackSource, SourceType};
use crate::spectrum_renderer::{self, SpectrumRenderer};

const BUFFER_SIZE: usize = 128 * 1024; // 128 KiB
const DEFAULT_DFT_WINDOW_SIZE: usize = 2048;

pub struct Controller {
	source: Option<Box<dyn JackSource>>,
	source_port_name: Option<String>,
	pubsub: PubSub,
	notifier: Notifier,
	graphic_renderer: AsyncProcessor<GraphicRenderer>,
	spectrum_renderer: AsyncProcessor<SpectrumRenderer>,
}

impl Controller {
	pub fn new() -> Result<Self, Error> {
		let pubsub = PubSub::new(None, glib::PRIORITY_DEFAULT);
		let notifier = pubsub.notifier();

		// Channel sending the spectrum from the spectrum rendering thread to the graphic rendering
		// thread.
		let (spectrum_graphic_tx, spectrum_graphic_rx) = mpsc::channel(0);
		// Channel sending the spectrum buffer from the graphic rendering thread to the spectrum
		// rendering thread.
		let (graphic_spectrum_tx, graphic_spectrum_rx) = mpsc::channel(0);

		// Start the graphic rendering background thread.
		let graphic_renderer = graphic_renderer::start(
			spectrum_graphic_rx,
			graphic_spectrum_tx,
		)?;

		// Start the spectrum rendering background thread.
		let spectrum_renderer = spectrum_renderer::start(
			graphic_spectrum_rx,
			spectrum_graphic_tx,
		)?;

		let mut controller = Controller {
			source: None,
			source_port_name: None,
			pubsub,
			notifier,
			graphic_renderer,
			spectrum_renderer,
		};
		controller.set_source_type(SourceType::Audio)?;
		Ok(controller)
	}

	pub fn pubsub(&self) -> &PubSub {
		&self.pubsub
	}

	pub fn get_source_type(&self) -> Option<SourceType> {
		self.source.as_ref().map(|source| source.source_type())
	}

	pub fn graphic_renderer(&self) -> &AsyncProcessor<GraphicRenderer> {
		&self.graphic_renderer
	}

	pub fn spectrum_renderer(&self) -> &AsyncProcessor<SpectrumRenderer> {
		&self.spectrum_renderer
	}

	pub fn set_source_type(&mut self, source_type: SourceType) -> Result<(), Error> {
		if Some(source_type) == self.get_source_type() {
			return Ok(());
		}
		debug!("Source type \"{}\" activated", source_type);

		// Drop old source first in case new source cannot be constructed.
		self.source = None;

		let (new_source, new_generator) = match source_type {
			SourceType::Audio => {
				let (source, reader) = AudioSourceController::new(
					BUFFER_SIZE, self.pubsub.notifier()
				)?;
				let sample_rate = source.client().sample_rate() as jack::Frames;
				let generator = AudioSpectrumGenerator::new(
					reader,
					sample_rate,
					DEFAULT_DFT_WINDOW_SIZE,
				);
				(Box::new(source), Box::new(generator))
			}
			SourceType::MIDI => unimplemented!("MIDI source is not yet implemented"),
		};
		self.source = Some(new_source);

		let mut spectrum_renderer = self.spectrum_renderer.clone();
		MainContext::default().spawn_local(async move {
			let result = spectrum_renderer
				.exec(move |renderer| renderer.set_generator(new_generator))
				.await;
			// TODO: Error handling.
			result.unwrap();
		});

		Ok(())
	}

	pub fn jack_client(&self) -> Option<&jack::Client> {
		self.source.as_ref().map(|source| source.client())
	}

	pub fn source_port_name(&self) -> Option<&str> {
		self.source_port_name.as_ref().map(AsRef::as_ref)
	}

	pub fn connect_port(&mut self, output_port: Option<String>) -> Result<(), Error> {
		if let Some(ref source) = self.source {
			let client = source.client();
			let input_port = source.input_port();

			client.disconnect(input_port)?;
			self.source_port_name = None;
			self.notify_and_log_err(events::SourcePortChanged);

			if let Some(output_port_name) = output_port {
				client.connect_ports_by_name(
					&output_port_name,
					&input_port.name()?
				)?;
				debug!("Connected port {}", output_port_name);
				self.source_port_name = Some(output_port_name);
				self.notify_and_log_err(events::SourcePortChanged);
			} else {
				debug!("Disconnected all ports");
			}

			Ok(())
		} else {
			Err(Error::NoJackSource)
		}
	}

	fn notify_and_log_err<T: Any + Send>(&self, notification: T) {
		if let Err(err) = self.notifier.send(notification) {
			log::error!("{}", Error::PubSub(err));
		}
	}

	pub async fn shutdown(&mut self) -> Result<(), Error> {
		self.source = None;
		self.spectrum_renderer.stop().await?;
		self.graphic_renderer.stop().await?;
		Ok(())
	}
}

pub mod events {
	pub struct SourcePortChanged;
}
