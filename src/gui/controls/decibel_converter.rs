use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::DecibelConverterController;
use crate::error::Error;
use crate::gui::handle_async_err;

const UI_DEF: &str = include_str!("decibel_converter.ui");

pub fn new(
	controller: &Rc<RefCell<DecibelConverterController>>,
) -> Result<impl IsA<gtk::Widget>, Error> {
	let builder = gtk::Builder::from_string(UI_DEF);
	let view: gtk::Frame = builder.object("toplevel").unwrap();
	let min_level_scale: gtk::Scale = builder.object("min_level_scale").unwrap();

	let controller_clone = controller.clone();
	min_level_scale.connect_change_value(move |_scale, _, value| {
		on_min_level_change(&controller_clone, value)
	});

	let controller = controller.borrow();
	let app_controller = controller.app_controller().borrow();
	let initial_value = match app_controller
		.config
		.spectrum_transforms
		.get(&controller.id())
	{
		Some(SpectrumTransformConfig::DecibelConverter(config)) => config.min_level,
		Some(_) => {
			return Err(Error::UnexpectedConfigEntry(format!(
				"expected decibel converter transform with id {}",
				controller.id()
			)))
		}
		None => {
			return Err(Error::MissingTransform {
				id: controller.id(),
			})
		}
	};

	min_level_scale.set_value(initial_value.log10());

	Ok(view)
}

fn on_min_level_change(
	controller: &Rc<RefCell<DecibelConverterController>>,
	value: f64,
) -> glib::Propagation {
	let mut controller = controller.borrow_mut();
	handle_async_err(controller.update_min_level(10.0f64.powf(value)));
	glib::Propagation::Proceed
}
