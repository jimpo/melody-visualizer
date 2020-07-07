use jack::PortId;
use futures::{prelude::*, channel::mpsc};
use log::{debug, error};
use std::cell::{RefCell, RefMut};
use std::mem;
use std::rc::Rc;
use std::sync::Arc;

use crate::audio::AudioSourceController;
use crate::error::Error;
use crate::graphic_renderer::{AsyncGraphicRenderer, Graphic, GraphicBuffer};
use crate::source::{JackSource, SourceSignals, SourceType};
use crate::spectral_renderer::AsyncSpectrumRenderer;

const BUFFER_SIZE: usize = 128 * 1024; // 128 KiB

// TODO: Wait for rendering threads on Drop.

pub struct Controller {
	source: Option<Box<dyn JackSource>>,
	signals: Arc<SourceSignals>,
	graphic_update_callbacks: Rc<RefCell<Vec<Box<dyn Fn()>>>>,
	inputs_changed_callbacks: Rc<RefCell<Vec<Box<dyn Fn(PortId)>>>>,
	graphic: Rc<RefCell<Graphic>>,
	graphic_renderer: AsyncGraphicRenderer,
	spectrum_renderer: AsyncSpectrumRenderer,
}

impl Controller {
	pub fn new() -> Result<Self, Error> {
		let inputs_changed_callbacks = Rc::new(RefCell::new(<Vec<Box<dyn Fn(PortId)>>>::new()));
		let graphic_update_callbacks = Rc::new(RefCell::new(<Vec<Box<dyn Fn()>>>::new()));

		let (inputs_changed_tx, inputs_changed_rx) =
			glib::MainContext::channel(glib::PRIORITY_DEFAULT);

		let signals = Arc::new(SourceSignals {
			on_inputs_changed: inputs_changed_tx,
		});

		let inputs_changed_callbacks_clone = inputs_changed_callbacks.clone();
		inputs_changed_rx.attach(None, move |port_id| {
			for callback in inputs_changed_callbacks_clone.borrow().iter() {
				callback(port_id);
			}
			glib::Continue(true)
		});

		// Create graphical rendering thread.

		let (source, buffer_reader) = AudioSourceController::new(BUFFER_SIZE, signals.clone())?;

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
		let graphic_renderer = AsyncGraphicRenderer::new(
			main_graphic_rx,
			graphic_main_tx,
			spectrum_graphic_rx,
			graphic_spectrum_tx,
		)?;

		// Start the spectrum rendering background thread.
		let spectrum_renderer = AsyncSpectrumRenderer::new(
			graphic_spectrum_rx,
			spectrum_graphic_tx,
		)?;

		let main_context = glib::MainContext::default();
		main_context.spawn_local(process_graphic_updates(
			graphic.clone(),
			graphic_update_callbacks.clone(),
			graphic_main_rx,
			main_graphic_tx,
		));

		Ok(Controller {
			source: Some(Box::new(source)),
			signals,
			inputs_changed_callbacks,
			graphic_update_callbacks,
			graphic,
			graphic_renderer,
			spectrum_renderer,
		})
	}

	pub fn graphic_mut(&mut self) -> RefMut<Graphic> {
		self.graphic.borrow_mut()
	}

	pub fn on_visualization_resize(x_max: i32, y_max: i32) {}

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

		let (new_source, reader) = match source_type {
			SourceType::Audio => {
				let (controller, reader) = AudioSourceController::new(
					BUFFER_SIZE, self.signals.clone()
				)?;
				(Box::new(controller), reader)
			}
			SourceType::MIDI => unimplemented!("MIDI source is not yet implemented"),
		};
		self.source = Some(new_source);

		Ok(())
	}

	// TODO: Maybe make this just return a Receiver<()>.
	pub fn subscribe_graphic_update(&mut self, callback: impl Fn() + 'static) {
		self.graphic_update_callbacks
			.borrow_mut()
			.push(Box::new(callback));
	}

	// TODO: Maybe make this just return a Receiver<PortId>.
	pub fn subscribe_inputs_changed(&mut self, callback: impl Fn(PortId) + 'static) {
		self.inputs_changed_callbacks
			.borrow_mut()
			.push(Box::new(callback));
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
	update_callbacks: Rc<RefCell<Vec<Box<dyn Fn()>>>>,
	mut graphic_rx: mpsc::Receiver<Graphic>,
	mut graphic_tx: mpsc::Sender<GraphicBuffer>,
) {
	// Kick things off by sending an empty graphic buffer.
	if let Err(err) = graphic_tx.send(GraphicBuffer::default()).await {
		error!("error sending initial graphic buffer to processing thread: {}", err);
		return;
	}

	while let Some(new_graphic) = graphic_rx.next().await {
		// Update the stored graphic.
		let old_graphic = mem::replace(&mut *graphic.borrow_mut(), new_graphic);

		// Notify subscribers that graphic has been updated. This triggers a redraw on the
		// visualization pane.
		for callback in update_callbacks.borrow().iter() {
			callback();
		}

		// Recycle the old graphic surface and send to renderer.
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
