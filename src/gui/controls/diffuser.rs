use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::DiffuserController;
use crate::error::Error;
use crate::gui::controls::captioned_slider::CaptionedSlider;
use crate::gui::handle_async_err;

const SEMITONES_PER_OCTAVE: f64 = 12.0;

/// The widest the slider offers, in semitones.
const MAX_WIDTH_SEMITONES: f64 = 10.0;

/// The width in the words the stage row and the control both use.
pub fn width_text(width: f64) -> String {
	format!("{:.1} semitones", width * SEMITONES_PER_OCTAVE)
}

pub fn new(
	controller: &Rc<RefCell<DiffuserController>>,
) -> Result<impl IsA<gtk::Widget> + use<>, Error> {
	let initial_width = {
		let controller = controller.borrow();
		let app_controller = controller.app_controller().borrow();
		match app_controller.config.spectrum_transform(controller.id())? {
			SpectrumTransformConfig::Diffuser(config) => config.width,
			config => {
				return Err(Error::UnexpectedConfigEntry(format!(
					"expected a Diffuser config for transform {}, found {:?}",
					controller.id(),
					config,
				)));
			}
		}
	};

	let adjustment = gtk::Adjustment::new(
		initial_width * SEMITONES_PER_OCTAVE,
		0.0,
		MAX_WIDTH_SEMITONES,
		1.0,
		MAX_WIDTH_SEMITONES,
		0.0,
	);
	let slider = CaptionedSlider::new(
		"Width",
		"Spreads each partial across neighbouring semitones.",
		&adjustment,
		|semitones| width_text(semitones / SEMITONES_PER_OCTAVE),
	);

	let controller_clone = controller.clone();
	slider
		.scale
		.connect_change_value(move |_scale, _, value| on_width_change(&controller_clone, value));

	let view = slider.widget;
	view.add_css_class("stage-body");
	Ok(view)
}

fn on_width_change(controller: &Rc<RefCell<DiffuserController>>, value: f64) -> glib::Propagation {
	let mut controller = controller.borrow_mut();
	handle_async_err(controller.update_width(value / SEMITONES_PER_OCTAVE));
	glib::Propagation::Proceed
}
