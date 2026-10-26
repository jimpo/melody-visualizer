use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::PowerMapController;
use crate::error::Error;
use crate::gui::controls::captioned_slider::CaptionedSlider;
use crate::gui::handle_async_err;

const MIN_EXPONENT: f64 = 0.5;
const MAX_EXPONENT: f64 = 4.0;

/// The exponent in the words the stage row and the control both use.
pub fn exponent_text(exponent: f64) -> String {
	format!("{exponent:.1}")
}

pub fn new(
	controller: &Rc<RefCell<PowerMapController>>,
) -> Result<impl IsA<gtk::Widget> + use<>, Error> {
	let initial_exponent = {
		let controller = controller.borrow();
		let app_controller = controller.app_controller().borrow();
		match app_controller.config.spectrum_transform(controller.id())? {
			SpectrumTransformConfig::PowerMap(config) => config.exponent,
			config => {
				return Err(Error::UnexpectedConfigEntry(format!(
					"expected a PowerMap config for transform {}, found {:?}",
					controller.id(),
					config,
				)));
			}
		}
	};

	let adjustment =
		gtk::Adjustment::new(initial_exponent, MIN_EXPONENT, MAX_EXPONENT, 0.1, 0.5, 0.0);
	let slider = CaptionedSlider::new(
		"Exponent",
		"Higher dims everything quieter than the loudest partial.",
		&adjustment,
		exponent_text,
	);

	let controller_clone = controller.clone();
	slider
		.scale
		.connect_change_value(move |_scale, _, value| on_exponent_change(&controller_clone, value));

	let view = slider.widget;
	view.add_css_class("stage-body");
	Ok(view)
}

fn on_exponent_change(
	controller: &Rc<RefCell<PowerMapController>>,
	value: f64,
) -> glib::Propagation {
	let mut controller = controller.borrow_mut();
	// The signal carries the value before the adjustment clamps it.
	handle_async_err(controller.update_exponent(value.clamp(MIN_EXPONENT, MAX_EXPONENT)));
	glib::Propagation::Proceed
}
