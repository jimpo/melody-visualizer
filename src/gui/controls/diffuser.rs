use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::DiffuserController;
use crate::error::Error;
use crate::gui::handle_async_err;

const SEMITONES_PER_OCTAVE: f64 = 12.0;
const UI_DEF: &str = include_str!("diffuser.ui");

pub fn new(controller: &Rc<RefCell<DiffuserController>>) -> Result<impl IsA<gtk::Widget>, Error> {
	let builder = gtk::Builder::from_string(UI_DEF);
	let view: gtk::Frame = builder.object("toplevel").unwrap();
	let width_scale: gtk::Scale = builder.object("width_scale").unwrap();

	let controller_clone = controller.clone();
	width_scale
		.connect_change_value(move |_scale, _, value| on_width_change(&controller_clone, value));

	let controller = controller.borrow();
	let app_controller = controller.app_controller().borrow();
	let initial_value = match app_controller
		.config
		.spectrum_transforms
		.get(&controller.id())
	{
		Some(SpectrumTransformConfig::Diffuser(config)) => config.width,
		Some(_) => {
			return Err(Error::UnexpectedConfigEntry(format!(
				"expected diffuser transform with id {}",
				controller.id()
			)))
		}
		None => {
			return Err(Error::MissingTransform {
				id: controller.id(),
			})
		}
	};

	width_scale.set_value(initial_value * SEMITONES_PER_OCTAVE);

	Ok(view)
}

fn on_width_change(controller: &Rc<RefCell<DiffuserController>>, value: f64) -> glib::Propagation {
	let mut controller = controller.borrow_mut();
	handle_async_err(controller.update_width(value / SEMITONES_PER_OCTAVE));
	glib::Propagation::Proceed
}
