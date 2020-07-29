use futures::{prelude::*, channel::mpsc, future::Either};
use glib::MainContext;
use log::{debug, error};
use std::{
	any::Any,
	cell::{RefCell, RefMut},
	mem,
	rc::Rc,
};

use crate::app::config::{Config, SpectrumGeneratorConfig};
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

pub struct Controller {
	source: Option<Box<dyn JackSource>>,
	source_port_name: Option<String>,
	config: Config,
	pubsub: PubSub,
	notifier: Notifier,
	graphic_renderer: AsyncProcessor<GraphicRenderer>,
	spectrum_renderer: AsyncProcessor<SpectrumRenderer>,
}

impl Controller {
	pub async fn new() -> Result<Rc<RefCell<Self>>, Error> {
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
			config: Config::default(),
			pubsub,
			notifier,
			graphic_renderer,
			spectrum_renderer,
		};
		controller.activate_source().await?;
		Ok(Rc::new(RefCell::new(controller)))
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

	fn activate_source(&mut self) -> impl Future<Output=Result<(), Error>> {
		// Drop old source first in case new source cannot be constructed.
		self.source = None;
		if self.source_port_name.is_some() {
			self.source_port_name = None;
			self.notify_and_log_err(events::SourcePortChanged);
		}

		let (new_source, new_generator) = match self.config.spectrum_generator {
			SpectrumGeneratorConfig::Audio(ref config) => {
				match AudioSourceController::new(BUFFER_SIZE, self.pubsub.notifier()) {
					Ok((source, reader)) => {
						debug!("Audio source activated");
						let sample_rate = source.client().sample_rate() as jack::Frames;
						let generator = AudioSpectrumGenerator::new(
							config.clone(), reader, sample_rate
						);
						(Box::new(source), Box::new(generator))
					}
					Err(err) => return Either::Left(future::ready(Err(err))),
				}
			}
		};
		self.source = Some(new_source);

		Either::Right(
			self.spectrum_renderer
				.exec_cloned(move |renderer| renderer.set_generator(new_generator))
				.map_err(Error::Communication)
		)
	}

	pub fn config(&self) -> &Config {
		&self.config
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
