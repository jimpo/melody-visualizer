use futures::{prelude::*, channel::mpsc, future::Either};
use std::{
	any::Any,
	cell::RefCell,
	rc::Rc,
};

use crate::app::config::{
	Config, GraphicGeneratorConfig, SpectrumGeneratorConfig, SpectrumTransformConfig,
};
use crate::audio::AudioSourceController;
use crate::spectrum::generators::audio::AudioSpectrumGenerator;
use crate::async_processor::AsyncProcessor;
use crate::error::Error;
use crate::graphic::renderer::{self, GraphicRenderer};
use crate::pubsub::{Notifier, PubSub};
use crate::source::{JackSource, SourceType};
use crate::spectrum::{
	renderer::{self as spectrum_processor, SpectrumRenderer},
	transforms::diffuser::Diffuser,
	transforms::volume_normalizer::VolumeNormalizer,
	SpectrumTransform,
};
use crate::graphic::generators::spiral::SpiralGenerator;

const BUFFER_SIZE: usize = 128 * 1024; // 128 KiB

pub struct AppController {
	pub config: Config,
	source: Option<Box<dyn JackSource>>,
	source_port_name: Option<String>,
	pubsub: PubSub,
	notifier: Notifier,
	graphic_renderer: AsyncProcessor<GraphicRenderer>,
	spectrum_renderer: AsyncProcessor<SpectrumRenderer>,
}

impl AppController {
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
		let graphic_renderer = renderer::start(
			spectrum_graphic_rx,
			graphic_spectrum_tx,
		)?;

		// Start the spectrum rendering background thread.
		let spectrum_renderer = spectrum_processor::start(
			graphic_spectrum_rx,
			spectrum_graphic_tx,
		)?;

		let mut controller = AppController {
			config: Config::default(),
			source: None,
			source_port_name: None,
			pubsub,
			notifier,
			graphic_renderer,
			spectrum_renderer,
		};
		controller.activate_source().await?;
		controller.sync_spectrum_transforms().await?;
		controller.update_spectrum_params().await?;
		controller.update_graphic_generator().await?;
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
						log::debug!("Audio source activated");
						let sample_rate = source.client().sample_rate() as jack::Frames;
						let generator = AudioSpectrumGenerator::new(
							config.clone(), reader, sample_rate
						);
						(Box::new(source), Box::new(generator))
					}
					Err(err) => return Either::Left(future::err(err)),
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

	pub fn update_graphic_generator(&self) -> impl Future<Output=Result<(), Error>> {
		let config = self.config.graphic_generator.clone();
		self.graphic_renderer
			.exec_cloned(move |renderer| {
				match config {
					GraphicGeneratorConfig::Spiral(config) => {
						match renderer
							.generator_mut()
							.upcast_any_mut()
							.downcast_mut::<SpiralGenerator>()
						{
							Some(spiral) => spiral.set_config(config),
							None => renderer.set_generator(Box::new(SpiralGenerator::new(config))),
						}
					}

				}
			})
			.map_err(Error::Communication)
	}

	pub fn update_spectrum_params(&self) -> impl Future<Output=Result<(), Error>> {
		let spectrum_params = self.config.spectrum_params();
		self.graphic_renderer
			.exec_cloned(move |renderer| {
				renderer.set_spectrum_params(spectrum_params);
			})
			.map_err(Error::Communication)
	}

	pub fn update_spectrum_transform(&self, id: u64) -> impl Future<Output=Result<(), Error>> {
		if let Some(config) = self.config.spectrum_transforms.get(&id) {
			let config = config.clone();
			let fut = self.spectrum_renderer
				.exec_cloned(move |renderer| {
					let transform = renderer.transforms_mut().get_mut(&id)
						.ok_or_else(|| Error::MissingTransform { id })?;
					match config {
						SpectrumTransformConfig::Diffuser(config) => {
							match transform
								.upcast_any_mut()
								.downcast_mut::<Diffuser>()
							{
								Some(transform) => transform.set_config(config),
								None => *transform = Box::new(Diffuser::new(config)),
							}
						}
						SpectrumTransformConfig::VolumeNormalizer(config) => {
							match transform
								.upcast_any_mut()
								.downcast_mut::<VolumeNormalizer>()
							{
								Some(transform) => transform.set_config(config),
								None => *transform = Box::new(VolumeNormalizer::new(config)),
							}
						}
					}
					Ok(())
				})
				.map(|result| {
					result
						.map_err(Error::Communication)
						.and_then(|result| result)
				});
			Either::Left(fut)
		} else {
			Either::Right(future::err(Error::MissingTransform { id }))
		}
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
				log::debug!("Connected port {}", output_port_name);
				self.source_port_name = Some(output_port_name);
				self.notify_and_log_err(events::SourcePortChanged);
			} else {
				log::debug!("Disconnected all ports");
			}

			Ok(())
		} else {
			Err(Error::NoJackSource)
		}
	}

	pub fn sync_spectrum_transforms(&self) -> impl Future<Output=Result<(), Error>> {
		let transform_configs = self.config.spectrum_transforms.clone();
		let transform_order = self.config.spectrum_transform_order.clone();
		self.spectrum_renderer
			.exec_cloned(move |renderer| {
				*renderer.transforms_mut() = transform_configs
					.into_iter()
					.map(|(id, config)| {
						let transform: Box<dyn SpectrumTransform> = match config {
							SpectrumTransformConfig::VolumeNormalizer(config) =>
								Box::new(VolumeNormalizer::new(config)),
							SpectrumTransformConfig::Diffuser(config) =>
								Box::new(Diffuser::new(config)),
						};
						(id, transform)
					})
					.collect();
				*renderer.transform_order_mut() = transform_order;
			})
			.map_err(Error::Communication)
	}

	pub fn insert_spectrum_transform(&mut self, transform_config: SpectrumTransformConfig)
		-> impl Future<Output=Result<(), Error>>
	{
		let id = self.config.unused_transform_id();
		self.config.spectrum_transforms.insert(id, transform_config.clone());
		self.config.spectrum_transform_order.push(id);
		let index = self.config.spectrum_transform_order.len() - 1;
		self.notify_and_log_err(events::InsertSpectrumTransform { index });

		let transform_order = self.config.spectrum_transform_order.clone();
		self.spectrum_renderer
			.exec_cloned(move |renderer| {
				let transform: Box<dyn SpectrumTransform> = match transform_config {
					SpectrumTransformConfig::VolumeNormalizer(config) =>
						Box::new(VolumeNormalizer::new(config)),
					SpectrumTransformConfig::Diffuser(config) =>
						Box::new(Diffuser::new(config)),
				};
				renderer.transforms_mut().insert(id, transform);
				*renderer.transform_order_mut() = transform_order;
			})
			.map_err(Error::Communication)
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
	#[derive(Debug, Clone)]
	pub struct SourcePortChanged;

	#[derive(Debug, Clone)]
	pub struct InsertSpectrumTransform {
		pub index: usize,
	}
}
