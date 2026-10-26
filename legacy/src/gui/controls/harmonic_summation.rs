use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::HarmonicSummationController;
use crate::error::Error;
use crate::gui::controls::GROUP_SPACING;
use crate::gui::controls::captioned_slider::CaptionedSlider;
use crate::gui::handle_async_err;
use crate::spectrum::transforms::harmonic_summation;

/// The most harmonics the slider offers.
const MAX_HARMONICS: usize = 16;

/// What the stage is set to, for its collapsed row.
pub fn summary(config: &harmonic_summation::Config) -> String {
	format!(
		"{} · {}",
		harmonics_text(config.harmonics),
		decay_text(config.decay)
	)
}

fn harmonics_text(harmonics: usize) -> String {
	format!("{harmonics} harmonics")
}

fn decay_text(decay: f64) -> String {
	format!("{decay:.2}")
}

/// The slider's value as a harmonic count the adjustment's range allows.
fn harmonics(value: f64) -> usize {
	value.round().clamp(1.0, MAX_HARMONICS as f64) as usize
}

pub fn new(
	controller: &Rc<RefCell<HarmonicSummationController>>,
) -> Result<impl IsA<gtk::Widget> + use<>, Error> {
	let initial = {
		let controller = controller.borrow();
		let app_controller = controller.app_controller().borrow();
		match app_controller.config.spectrum_transform(controller.id())? {
			SpectrumTransformConfig::HarmonicSummation(config) => config.clone(),
			config => {
				return Err(Error::UnexpectedConfigEntry(format!(
					"expected a HarmonicSummation config for transform {}, found {:?}",
					controller.id(),
					config,
				)));
			}
		}
	};

	let harmonics_slider = CaptionedSlider::new(
		"Harmonics",
		"How many multiples of its frequency each partial collects, itself included.",
		&gtk::Adjustment::new(
			initial.harmonics as f64,
			1.0,
			MAX_HARMONICS as f64,
			1.0,
			4.0,
			0.0,
		),
		|value| harmonics_text(harmonics(value)),
	);
	let controller_clone = controller.clone();
	harmonics_slider
		.scale
		.connect_change_value(move |_scale, _, value| {
			handle_async_err(
				controller_clone
					.borrow_mut()
					.update(move |config| config.harmonics = harmonics(value)),
			);
			glib::Propagation::Proceed
		});

	let decay_slider = CaptionedSlider::new(
		"Decay",
		"The weight of each harmonic against the one below it.",
		&gtk::Adjustment::new(initial.decay, 0.0, 1.0, 0.05, 0.1, 0.0),
		decay_text,
	);
	let controller_clone = controller.clone();
	decay_slider
		.scale
		.connect_change_value(move |_scale, _, value| {
			let decay = value.clamp(0.0, 1.0);
			handle_async_err(
				controller_clone
					.borrow_mut()
					.update(move |config| config.decay = decay),
			);
			glib::Propagation::Proceed
		});

	let view = gtk::Box::new(gtk::Orientation::Vertical, GROUP_SPACING);
	view.add_css_class("stage-body");
	view.append(&harmonics_slider.widget);
	view.append(&decay_slider.widget);
	Ok(view)
}
