use cairo;
use jack::PortId;
use log::{debug, error};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::audio::AudioSourceController;
use crate::error::Error;
use crate::source::{JackSource, SourceSignals, SourceType};
use glib::Continue;

const BUFFER_SIZE: usize = 128 * 1024; // 128 KiB


pub struct Controller {
	source: Option<Box<dyn JackSource>>,
	signals: Arc<SourceSignals>,
	inputs_changed_callbacks: Rc<RefCell<Vec<Box<dyn Fn(PortId)>>>>,
	graphic: Option<cairo::ImageSurface>,
}

impl Controller {
	pub fn new() -> Result<Self, Error> {
		let (inputs_changed_tx, inputs_changed_rx) =
			glib::MainContext::channel(glib::PRIORITY_DEFAULT);

		let signals = Arc::new(SourceSignals {
			on_inputs_changed: inputs_changed_tx,
		});
		let source = Box::new(AudioSourceController::new(BUFFER_SIZE, signals.clone())?);
		let inputs_changed_callbacks = Rc::new(RefCell::new(<Vec<Box<dyn Fn(PortId)>>>::new()));

		let inputs_changed_callbacks_clone = inputs_changed_callbacks.clone();
		inputs_changed_rx.attach(None, move |port_id| {
			for callback in inputs_changed_callbacks_clone.borrow_mut().iter() {
				callback(port_id);
			}
			Continue(true)
		});

		Ok(Controller {
			source: Some(source),
			signals,
			inputs_changed_callbacks,
			graphic: None,
		})
	}

	pub fn graphic_mut(&mut self) -> &mut Option<cairo::ImageSurface> {
		&mut self.graphic
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

		let new_source = match source_type {
			SourceType::Audio => {
				Box::new(AudioSourceController::new(BUFFER_SIZE, self.signals.clone())?)
			}
			SourceType::MIDI => unimplemented!("MIDI source is not yet implemented"),
		};
		self.source = Some(new_source);

		Ok(())
	}

	pub fn subscribe_inputs_changed(&mut self, callback: Box<dyn Fn(PortId)>) {
		self.inputs_changed_callbacks
			.borrow_mut()
			.push(callback);
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