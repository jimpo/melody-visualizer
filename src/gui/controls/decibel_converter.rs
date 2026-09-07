use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::DecibelConverterController;
use crate::error::Error;
use crate::gui::controls::captioned_slider::CaptionedSlider;
use crate::gui::handle_async_err;

/// The slider moves the floor in decades: its value is log₁₀ of the level.
const MIN_DECADES: f64 = -10.0;
const MAX_DECADES: f64 = 10.0;

/// The floor in the words the stage row and the control both use.
///
/// Typeset with a real minus sign, since the value is nearly always negative.
pub fn floor_text(min_level: f64) -> String {
	format!("{:.0} dB", 10.0 * min_level.log10()).replace('-', "−")
}

pub fn new(
	controller: &Rc<RefCell<DecibelConverterController>>,
) -> Result<impl IsA<gtk::Widget> + use<>, Error> {
	let initial_level = {
		let controller = controller.borrow();
		let app_controller = controller.app_controller().borrow();
		match app_controller.config.spectrum_transform(controller.id())? {
			SpectrumTransformConfig::DecibelConverter(config) => config.min_level,
			config => {
				return Err(Error::UnexpectedConfigEntry(format!(
					"expected a DecibelConverter config for transform {}, found {:?}",
					controller.id(),
					config,
				)));
			}
		}
	};

	let adjustment = gtk::Adjustment::new(
		initial_level.log10(),
		MIN_DECADES,
		MAX_DECADES,
		1.0,
		MAX_DECADES,
		0.0,
	);
	let slider = CaptionedSlider::new(
		"Floor",
		"Anything this far under the peak is drawn black.",
		&adjustment,
		|decades| floor_text(10.0f64.powf(decades)),
	);

	let controller_clone = controller.clone();
	slider.scale.connect_change_value(move |_scale, _, value| {
		on_min_level_change(&controller_clone, value)
	});

	let view = slider.widget;
	view.add_css_class("stage-body");
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
