use gtk::prelude::*;
use std::{
	cell::RefCell,
	rc::Rc,
};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::VolumeNormalizerController;
use crate::error::Error;
use crate::gui::handle_async_err;

const UI_DEF: &str = include_str!("volume_normalizer.ui");

pub fn new(controller: &Rc<RefCell<VolumeNormalizerController>>)
	-> Result<impl IsA<gtk::Widget>, Error>
{
	let builder = gtk::Builder::from_string(UI_DEF);
	let view: gtk::Frame = builder.get_object("toplevel").unwrap();
	let rate_scale: gtk::Scale = builder.get_object("rate_scale").unwrap();

	let controller_clone = controller.clone();
	rate_scale.connect_change_value(
		move |_scale, _, value| on_rate_change(&controller_clone, value)
	);

	let controller = controller.borrow();
	let app_controller = controller.app_controller().borrow();
	let initial_value = match app_controller.config.spectrum_transforms.get(&controller.id()) {
		Some(SpectrumTransformConfig::VolumeNormalizer(config)) => config.rate,
		Some(_) => return Err(Error::UnexpectedConfigEntry(format!(
			"expected volume normalizer transform with id {}", controller.id()
		))),
		None => return Err(Error::MissingTransform { id: controller.id() }),
	};

	rate_scale.set_value(initial_value);

	Ok(view)
}

fn on_rate_change(controller: &Rc<RefCell<VolumeNormalizerController>>, value: f64) -> Inhibit {
	let mut controller = controller.borrow_mut();
	handle_async_err(controller.update_rate(value));
	Inhibit(false)
}
