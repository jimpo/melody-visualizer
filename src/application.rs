use cairo;
use jack::PortId;
use log::debug;
use std::cell::RefCell;
use std::rc::Rc;

use crate::audio::AudioSourceController;
use crate::error::Error;
use glib::Continue;

const BUFFER_SIZE: usize = 128 * 1024; // 128 KiB

#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum SourceType {
	Audio,
	MIDI,
}

pub struct Controller {
	source_type: SourceType,
	source: AudioSourceController,
	inputs_changed_callbacks: Rc<RefCell<Vec<Box<dyn Fn(PortId)>>>>,
	graphic: Option<cairo::ImageSurface>,
}

impl Controller {
	pub fn new() -> Result<Self, Error> {
		let source_type = SourceType::Audio;

		let (inputs_changed_tx, inputs_changed_rx) =
			glib::MainContext::channel(glib::PRIORITY_DEFAULT);

		let source = AudioSourceController::new(BUFFER_SIZE, inputs_changed_tx)?;
		let inputs_changed_callbacks = Rc::new(RefCell::new(<Vec<Box<dyn Fn(PortId)>>>::new()));

		let inputs_changed_callbacks_clone = inputs_changed_callbacks.clone();
		inputs_changed_rx.attach(None, move |port_id| {
			for callback in inputs_changed_callbacks_clone.borrow_mut().iter() {
				callback(port_id);
			}
			Continue(true)
		});

		Ok(Controller {
			source_type,
			source,
			inputs_changed_callbacks,
			graphic: None,
		})
	}

	pub fn graphic_mut(&mut self) -> &mut Option<cairo::ImageSurface> {
		&mut self.graphic
	}

	pub fn on_visualization_resize(x_max: i32, y_max: i32) {}

	pub fn get_source_type(&self) -> SourceType {
		self.source_type
	}

	pub fn set_source_type(&mut self, source_type: SourceType) {
		if source_type == self.source_type {
			return;
		}
		debug!("Source type \"{}\" activated", source_type);

		self.source_type = source_type;
	}

	pub fn subscribe_inputs_changed(&mut self, callback: Box<dyn Fn(PortId)>) {
		self.inputs_changed_callbacks
			.borrow_mut()
			.push(callback);
	}

	pub fn jack_client(&self) -> &jack::Client {
		self.source.client()
	}
}