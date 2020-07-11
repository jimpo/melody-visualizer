use futures::{prelude::*, channel::mpsc};
use log::{debug, error};
use std::cell::{RefCell, RefMut};
use std::mem;
use std::rc::Rc;

use crate::audio::AudioSourceController;
use crate::audio_spectrum_generator::AudioSpectrumGenerator;
use crate::async_processor::AsyncProcessor;
use crate::error::Error;
use crate::graphic::{Graphic, GraphicBuffer};
use crate::graphic_renderer::{self, GraphicRendererCmd};
use crate::pubsub::{Notifier, PubSub};
use crate::source::{JackSource, SourceType};
use crate::spectrum_renderer::{self, SpectrumRendererCmd};
use glib::MainContext;

const BUFFER_SIZE: usize = 128 * 1024; // 128 KiB

// TODO: Wait for rendering threads on Drop.

pub struct Controller {
	source: Option<Box<dyn JackSource>>,
	pubsub: PubSub,
	graphic: Rc<RefCell<Graphic>>,
	graphic_renderer: AsyncProcessor<GraphicRendererCmd>,
	spectrum_renderer: AsyncProcessor<SpectrumRendererCmd>,
}

impl Controller {
	pub fn new() -> Result<Self, Error> {
		let pubsub = PubSub::new(None, glib::PRIORITY_DEFAULT);

		// Create graphical rendering thread.

		// Create spectral rendering thread.
		// - Channel<Spectrum> in
		// - Command SpectrumPipeline
		// - Channel<Spectrum> out

		let graphic = Rc::new(RefCell::new(Graphic::default()));

		// Channel sending the graphic surface from the main thread to the graphic rendering thread.
		let (main_graphic_tx, main_graphic_rx) = mpsc::channel(0);
		// Channel sending the graphic surface from the graphic rendering thread to the main thread.
		let (graphic_main_tx, graphic_main_rx) = mpsc::channel(0);

		// Channel sending the spectrum from the spectrum rendering thread to the graphic rendering
		// thread.
		let (spectrum_graphic_tx, spectrum_graphic_rx) = mpsc::channel(0);
		// Channel sending the spectrum buffer from the graphic rendering thread to the spectrum
		// rendering thread.
		let (graphic_spectrum_tx, graphic_spectrum_rx) = mpsc::channel(0);

		// Start the graphic rendering background thread.
		let graphic_renderer = graphic_renderer::start(
			main_graphic_rx,
			graphic_main_tx,
			spectrum_graphic_rx,
			graphic_spectrum_tx,
		)?;

		// Start the spectrum rendering background thread.
		let spectrum_renderer = spectrum_renderer::start(
			graphic_spectrum_rx,
			spectrum_graphic_tx,
		)?;

		let main_context = glib::MainContext::default();
		main_context.spawn_local(process_graphic_updates(
			graphic.clone(),
			pubsub.notifier(),
			graphic_main_rx,
			main_graphic_tx,
		));

		let mut controller = Controller {
			source: None,
			graphic,
			pubsub,
			graphic_renderer,
			spectrum_renderer,
		};
		controller.set_source_type(SourceType::Audio)?;
		Ok(controller)
	}

	pub fn pubsub(&self) -> &PubSub {
		&self.pubsub
	}

	pub fn graphic_mut(&mut self) -> RefMut<Graphic> {
		self.graphic.borrow_mut()
	}

	pub fn get_source_type(&self) -> Option<SourceType> {
		self.source.as_ref().map(|source| source.source_type())
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
				);
				(Box::new(source), Box::new(generator))
			}
			SourceType::MIDI => unimplemented!("MIDI source is not yet implemented"),
		};
		self.source = Some(new_source);

		let mut spectrum_renderer = self.spectrum_renderer.clone();
		MainContext::default().spawn_local(async move {
			let _  = spectrum_renderer.call::<()>(SpectrumRendererCmd::SetGenerator(new_generator))
				.await;
		});

		Ok(())
	}

	pub fn jack_client(&self) -> Option<&jack::Client> {
		self.source.as_ref().map(|source| source.client())
	}

	pub fn connect_port(&mut self, output_port: Option<String>) {
		match self.source {
			Some(ref source) => {
				if let Err(err) = connect_port(&**source, output_port) {
					error!("connect_port: {}", err);
				}
			}
			None => error!("connect_port called with empty source"),
		}
	}

	pub async fn shutdown(&mut self) -> Result<(), Error> {
		self.source = None;
		self.spectrum_renderer.stop().await?;
		self.graphic_renderer.stop().await?;
		Ok(())
	}
}

fn connect_port(source: &dyn JackSource, output_port: Option<String>) -> Result<(), Error> {
	let client = source.client();
	let input_port = source.input_port();
	client.disconnect(input_port)?;
	if let Some(output_port_name) = output_port {
		client.connect_ports_by_name(
			&output_port_name,
			&input_port.name()?
		)?;
		debug!("Connected port {}", output_port_name);
	} else {
		debug!("Disconnected all ports");
	}
	Ok(())
}

async fn process_graphic_updates(
	graphic: Rc<RefCell<Graphic>>,
	notifier: Notifier,
	mut graphic_rx: mpsc::Receiver<Graphic>,
	mut graphic_tx: mpsc::Sender<GraphicBuffer>,
) {
	while let Some(new_graphic) = graphic_rx.next().await {
		// Update the stored graphic.
		let old_graphic = mem::replace(&mut *graphic.borrow_mut(), new_graphic);

		// Notify subscribers that graphic has been updated. This triggers a redraw on the
		// visualization pane.
		if let Err(err) = notifier.send(events::GraphicUpdate) {
			error!("failed to notify of graphic update: {}", err);
		}

		// Recycle the old graphic and send empty buffer to renderer.
		if let Err(err) = graphic_tx.send(old_graphic.into_buffer()).await {
			if err.is_disconnected() {
				debug!("graphic output channel disconnected, stopping main thread handler");
				break;
			} else {
				error!("error sending graphic buffer to processing thread");
			}
		}
	}
}

pub mod events {
	pub struct GraphicUpdate;
}
