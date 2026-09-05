use async_channel::Receiver;
use futures::{channel::mpsc, future::Either, prelude::*};
use std::{any::Any, cell::RefCell, rc::Rc};

use crate::app::config::{Config, SpectrumGeneratorConfig};
use crate::async_processor::AsyncProcessor;
use crate::audio::AudioSource;
use crate::audio::source::{
	JackSource, PortName, SourceType,
	events::{Event as AudioSourceEvent, SampleRateChanged},
};
use crate::error::Error;
use crate::graphic::renderer::{self, GraphicRenderer};
use crate::pubsub::{Notifier, PubSub, SubscriptionHandle};
use crate::spectrum::TransformId;
use crate::spectrum::generators::audio::AudioSpectrumGenerator;
use crate::spectrum::renderer::{self as spectrum_processor, SpectrumRenderer};

const BUFFER_SIZE: usize = 128 * 1024; // 128 KiB

pub struct AppController {
	pub config: Config,
	source: Option<Box<dyn JackSource>>,
	pubsub: PubSub,
	notifier: Notifier,
	graphic_renderer: AsyncProcessor<GraphicRenderer>,
	spectrum_renderer: AsyncProcessor<SpectrumRenderer>,
	_sample_rate_subscription: SubscriptionHandle,
}

impl AppController {
	pub async fn new() -> Result<Rc<RefCell<Self>>, Error> {
		let pubsub = PubSub::new(None, glib::Priority::DEFAULT);
		let notifier = pubsub.notifier();

		// Channel sending the spectrum from the spectrum rendering thread to the graphic rendering
		// thread.
		let (spectrum_graphic_tx, spectrum_graphic_rx) = mpsc::channel(0);
		// Channel sending the spectrum buffer from the graphic rendering thread to the spectrum
		// rendering thread.
		let (graphic_spectrum_tx, graphic_spectrum_rx) = mpsc::channel(0);

		// Start the graphic rendering background thread.
		let graphic_renderer = renderer::start(spectrum_graphic_rx, graphic_spectrum_tx)?;

		// Start the spectrum rendering background thread.
		let spectrum_renderer =
			spectrum_processor::start(graphic_spectrum_rx, spectrum_graphic_tx)?;

		let sample_rate_subscription = subscribe_to_sample_rate(&pubsub, spectrum_renderer.clone());
		let mut controller = AppController {
			config: Config::default(),
			source: None,
			pubsub,
			notifier,
			graphic_renderer,
			spectrum_renderer,
			_sample_rate_subscription: sample_rate_subscription,
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

	fn activate_source(&mut self) -> impl Future<Output = Result<(), Error>> + use<> {
		// Drop old source first in case new source cannot be constructed.
		self.source = None;

		let (new_source, new_generator) = match self.config.spectrum_generator {
			SpectrumGeneratorConfig::Audio(ref config) => match AudioSource::new(BUFFER_SIZE) {
				Ok((source, reader, events)) => {
					log::debug!("Audio source activated");
					republish_audio_events(events, self.pubsub.notifier());
					let sample_rate = source.sample_rate();
					let generator =
						AudioSpectrumGenerator::new(config.clone(), reader, sample_rate);
					(Box::new(source), Box::new(generator))
				}
				Err(err) => return Either::Left(future::err(err)),
			},
		};
		self.source = Some(new_source);

		Either::Right(
			self.spectrum_renderer
				.exec_cloned(move |renderer| renderer.set_generator(new_generator))
				.map_err(Error::Communication),
		)
	}

	pub fn update_graphic_generator(&self) -> impl Future<Output = Result<(), Error>> + use<> {
		let config = self.config.graphic_generator.clone();
		self.notify_and_log_err(events::ConfigChanged);
		self.graphic_renderer
			.exec_cloned(move |renderer| {
				renderer.update_generator(|generator| config.update(generator));
			})
			.map_err(Error::Communication)
	}

	pub fn update_spectrum_params(&self) -> impl Future<Output = Result<(), Error>> + use<> {
		let spectrum_params = self.config.spectrum_params();
		self.notify_and_log_err(events::ConfigChanged);
		self.graphic_renderer
			.exec_cloned(move |renderer| {
				renderer.set_spectrum_params(spectrum_params);
			})
			.map_err(Error::Communication)
	}

	pub fn update_spectrum_transform(
		&self,
		id: TransformId,
	) -> impl Future<Output = Result<(), Error>> + use<> {
		let config = match self.config.spectrum_transform(id) {
			Ok(config) => config.clone(),
			Err(err) => return Either::Right(future::err(err)),
		};
		self.notify_and_log_err(events::ConfigChanged);
		let fut = self
			.spectrum_renderer
			.exec_cloned(move |renderer| {
				let transform = renderer
					.transforms_mut()
					.get_mut(id)
					.ok_or(Error::MissingTransform { id })?;
				config.update(transform)
			})
			.map(|result| {
				result
					.map_err(Error::Communication)
					.and_then(|result| result)
			});
		Either::Left(fut)
	}

	/// The ports that can be connected to the source's input, as JACK reports
	/// them right now. Empty when there is no source.
	pub fn available_inputs(&self) -> Vec<PortName> {
		self.source
			.as_ref()
			.map(|source| source.available_inputs())
			.unwrap_or_default()
	}

	/// The port feeding the source's input, as JACK reports it now. Nothing is
	/// cached, so a connection made outside the app is reported too.
	pub fn connected_input(&self) -> Option<PortName> {
		self.source
			.as_ref()
			.and_then(|source| source.connected_input())
	}

	/// Feed the source's input from `output_port`, or from nothing at all.
	///
	/// The change is announced by JACK, not from here: the server calls back
	/// with a [`ConnectionChanged`](crate::audio::source::events::ConnectionChanged)
	/// once the graph really holds it.
	pub fn connect_port(&self, output_port: Option<PortName>) -> Result<(), Error> {
		let source = self.source.as_ref().ok_or(Error::NoJackSource)?;
		match output_port {
			Some(output_port) => {
				source.connect(&output_port)?;
				log::debug!("Connected port {}", output_port);
			}
			None => {
				source.disconnect()?;
				log::debug!("Disconnected all ports");
			}
		}
		Ok(())
	}

	pub fn sync_spectrum_transforms(&self) -> impl Future<Output = Result<(), Error>> + use<> {
		let transform_configs = self.config.spectrum_transforms.clone();
		self.spectrum_renderer
			.exec_cloned(move |renderer| {
				*renderer.transforms_mut() = transform_configs
					.into_iter()
					.map(|(id, config)| (id, config.create()))
					.collect();
			})
			.map_err(Error::Communication)
	}

	/// Publishes `notification`, logging a failure rather than propagating it.
	///
	/// A send fails only when the bus itself is gone, which is not a condition
	/// the caller can act on.
	fn notify_and_log_err<T: Any + Send>(&self, notification: T) {
		if let Err(err) = self.notifier.send(notification) {
			log::error!("{}", Error::PubSub(err));
		}
	}

	/// Drops the JACK source and stops both renderer threads.
	///
	/// Stopping a renderer closes its command channel, which ends its loop; the
	/// thread then winds down on its own. Nothing here waits for it, so the GTK
	/// main loop is never blocked.
	pub fn shutdown(&mut self) {
		self.source = None;
		self.spectrum_renderer.stop();
		self.graphic_renderer.stop();
	}
}

/// Forward JACK rate changes from the GTK event bus to the spectrum thread.
fn subscribe_to_sample_rate(
	pubsub: &PubSub,
	spectrum_renderer: AsyncProcessor<SpectrumRenderer>,
) -> SubscriptionHandle {
	pubsub.subscribe(move |&SampleRateChanged(sample_rate)| {
		let update = spectrum_renderer.exec_cloned(move |renderer| {
			renderer.generator_mut().set_sample_rate(sample_rate);
		});
		glib::MainContext::ref_thread_default().spawn_local(async move {
			if let Err(err) = update.await {
				log::error!("failed to update DSP sample rate: {err}");
			}
		});
	})
}

/// Republish the events an audio source reports onto the app-wide bus.
///
/// This task is the one place the audio module and PubSub meet: the source
/// reports on a plain channel, and each event goes out to the subscribers as its
/// own type. It runs on the GTK main context and ends when the source is
/// dropped, which closes the channel.
fn republish_audio_events(events: Receiver<AudioSourceEvent>, notifier: Notifier) {
	// Dropping the returned JoinHandle detaches the task; it keeps running until
	// the channel closes.
	glib::MainContext::ref_thread_default().spawn_local(async move {
		while let Ok(event) = events.recv().await {
			let published = match event {
				AudioSourceEvent::PortsChanged(event) => notifier.send(event),
				AudioSourceEvent::ConnectionChanged(event) => notifier.send(event),
				AudioSourceEvent::SampleRateChanged(event) => notifier.send(event),
				AudioSourceEvent::ServerShutdown(event) => notifier.send(event),
			};
			if let Err(err) = published {
				log::error!("{}", Error::PubSub(err));
			}
		}
	});
}

pub mod events {
	/// A field of [`Config`](crate::app::config::Config) holds a new value, and
	/// the threads that act on it are being told.
	///
	/// The views that report a config value subscribe to this rather than to a
	/// notification per field, so a summary in the control pane stays current
	/// whichever control moved.
	#[derive(Debug, Clone)]
	pub struct ConfigChanged;
}
