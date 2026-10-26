use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::VolumeNormalizerController;
use crate::error::Error;
use crate::gui::controls::captioned_slider::CaptionedSlider;
use crate::gui::handle_async_err;

/// The slowest rate the slider offers. At 0 the running peak could never move.
const MIN_RATE: f64 = 0.01;
const MAX_RATE: f64 = 1.0;

/// The rate in the words the stage row and the control both use.
pub fn rate_text(rate: f64) -> String {
	format!("{rate:.2}")
}

pub fn new(
	controller: &Rc<RefCell<VolumeNormalizerController>>,
) -> Result<impl IsA<gtk::Widget> + use<>, Error> {
	let initial_rate = {
		let controller = controller.borrow();
		let app_controller = controller.app_controller().borrow();
		match app_controller.config.spectrum_transform(controller.id())? {
			SpectrumTransformConfig::VolumeNormalizer(config) => config.rate,
			config => {
				return Err(Error::UnexpectedConfigEntry(format!(
					"expected a VolumeNormalizer config for transform {}, found {:?}",
					controller.id(),
					config,
				)));
			}
		}
	};

	let adjustment = gtk::Adjustment::new(initial_rate, MIN_RATE, MAX_RATE, 0.1, MAX_RATE, 0.0);
	let slider = CaptionedSlider::new(
		"Rate",
		"How fast the running peak follows the loudest partial.",
		&adjustment,
		rate_text,
	);

	let controller_clone = controller.clone();
	slider
		.scale
		.connect_change_value(move |_scale, _, value| on_rate_change(&controller_clone, value));

	let view = slider.widget;
	view.add_css_class("stage-body");
	Ok(view)
}

fn on_rate_change(
	controller: &Rc<RefCell<VolumeNormalizerController>>,
	value: f64,
) -> glib::Propagation {
	let mut controller = controller.borrow_mut();
	handle_async_err(controller.update_rate(value));
	glib::Propagation::Proceed
}
