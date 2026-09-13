//! The spectrum stage's body: how much audio each spectrum analyses, and how
//! often it produces one.

use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumGeneratorConfig;
use crate::controllers::AppController;
use crate::gui::controls::GROUP_SPACING;
use crate::gui::controls::captioned_slider::CaptionedSlider;
use crate::gui::handle_async_err;
use crate::spectrum::generators::audio;

pub fn new(app_controller: &Rc<RefCell<AppController>>) -> gtk::Box {
	let body = gtk::Box::new(gtk::Orientation::Vertical, GROUP_SPACING);
	body.add_css_class("stage-body");
	body.append(&build_window(app_controller));
	body.append(&build_update_rate(app_controller));
	body
}

fn build_window(app_controller: &Rc<RefCell<AppController>>) -> gtk::Box {
	let window_ms = audio_config(&app_controller.borrow()).window_ms;
	let slider = whole_number_slider(
		"Window",
		"The audio each spectrum analyses. Longer separates pitches better and reacts later.",
		window_ms,
		audio::MIN_WINDOW_MS,
		audio::MAX_WINDOW_MS,
		window_text,
	);

	let controller = app_controller.clone();
	slider.scale.connect_change_value(move |_scale, _, value| {
		let window_ms = clamp_round(value, audio::MIN_WINDOW_MS, audio::MAX_WINDOW_MS);
		// A drag crosses many pixels per millisecond, and a new window size
		// re-plans the DFT.
		update_audio_config(&controller, |config| {
			std::mem::replace(&mut config.window_ms, window_ms) != window_ms
		});
		glib::Propagation::Proceed
	});

	slider.widget
}

fn build_update_rate(app_controller: &Rc<RefCell<AppController>>) -> gtk::Box {
	let update_rate = audio_config(&app_controller.borrow()).update_rate;
	let slider = whole_number_slider(
		"Update rate",
		"How often a new spectrum is produced, whatever the window.",
		update_rate,
		audio::MIN_UPDATE_RATE,
		audio::MAX_UPDATE_RATE,
		rate_text,
	);

	let controller = app_controller.clone();
	slider.scale.connect_change_value(move |_scale, _, value| {
		let update_rate = clamp_round(value, audio::MIN_UPDATE_RATE, audio::MAX_UPDATE_RATE);
		update_audio_config(&controller, |config| {
			std::mem::replace(&mut config.update_rate, update_rate) != update_rate
		});
		glib::Propagation::Proceed
	});

	slider.widget
}

/// A slider over whole numbers from `min` to `max`, printing its value with
/// `format`.
///
/// The value label rounds the way the handlers do, so the two never name
/// different numbers either side of a half.
fn whole_number_slider(
	label: &str,
	caption: &str,
	value: u32,
	min: u32,
	max: u32,
	format: fn(u32) -> String,
) -> CaptionedSlider {
	let adjustment = gtk::Adjustment::new(
		value as f64,
		min as f64,
		max as f64,
		1.0,
		(max - min) as f64 / 10.0,
		0.0,
	);
	CaptionedSlider::new(label, caption, &adjustment, move |value| {
		format(clamp_round(value, min, max))
	})
}

/// The value a scale is being moved to, which the range has yet to clamp to its
/// own bounds, and which a jump lands between two whole numbers.
fn clamp_round(value: f64, min: u32, max: u32) -> u32 {
	value.round().clamp(min as f64, max as f64) as u32
}

/// Applies `change` to the generator config, and ships it to the spectrum
/// thread when `change` reports it moved a value.
fn update_audio_config(
	app_controller: &Rc<RefCell<AppController>>,
	change: impl FnOnce(&mut audio::Config) -> bool,
) {
	let mut app_controller = app_controller.borrow_mut();
	let SpectrumGeneratorConfig::Audio(config) = &mut app_controller.config.spectrum_generator;
	if change(config) {
		handle_async_err(app_controller.update_spectrum_generator());
	}
}

/// The window in the words the stage row and the control both use.
fn window_text(window_ms: u32) -> String {
	format!("{window_ms} ms")
}

/// The update rate in the words the stage row and the control both use.
fn rate_text(update_rate: u32) -> String {
	format!("{update_rate} / sec")
}

/// What the stage is set to, for its collapsed row.
pub fn summary(app_controller: &AppController) -> String {
	let config = audio_config(app_controller);
	format!(
		"{} · {}",
		window_text(config.window_ms),
		rate_text(config.update_rate)
	)
}

fn audio_config(app_controller: &AppController) -> &audio::Config {
	let SpectrumGeneratorConfig::Audio(config) = &app_controller.config.spectrum_generator;
	config
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_default_generator_is_within_reach_of_the_sliders() {
		// The adjustment clamps a value outside its range, and nothing writes
		// that back, so a default out of reach leaves a slider reporting a
		// value the spectrum is not analysed at.
		let config = audio::Config::default();
		assert!((audio::MIN_WINDOW_MS..=audio::MAX_WINDOW_MS).contains(&config.window_ms));
		assert!((audio::MIN_UPDATE_RATE..=audio::MAX_UPDATE_RATE).contains(&config.update_rate));
	}
}
