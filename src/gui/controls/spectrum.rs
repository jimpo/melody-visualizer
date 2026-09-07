//! The spectrum stage's body: the grid it analyses on, and how often it
//! produces a spectrum.

use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumGeneratorConfig;
use crate::controllers::AppController;
use crate::gui::controls::captioned_slider::CaptionedSlider;
use crate::gui::controls::{GROUP_SPACING, caption};
use crate::gui::handle_async_err;
use crate::spectrum::generators::audio;

/// The rate the labels read at while no source is open to report one.
const FALLBACK_SAMPLE_RATE: u32 = 48_000;

/// The most of a window the slider offers to repeat, as a percentage.
///
/// The hop is what is left of the window, so the rate the slider sets climbs
/// ever more steeply towards a whole window overlapping, for ever less of the
/// audio being new.
const MAX_OVERLAP_PERCENT: f64 = 75.0;

/// The step the slider moves the overlap in, as a percentage.
const OVERLAP_STEP_PERCENT: f64 = 5.0;

pub fn new(app_controller: &Rc<RefCell<AppController>>) -> gtk::Box {
	let body = gtk::Box::new(gtk::Orientation::Vertical, GROUP_SPACING);
	body.add_css_class("stage-body");
	body.append(&caption(&grid_text(&app_controller.borrow())));
	body.append(&build_update_rate(app_controller));
	body
}

/// The update rate group: the rate the spectrum advances at, over a slider
/// that moves the overlap the rate comes from.
///
/// The rate is what a listener hears and the overlap is what the DFT does, so
/// the rate names the control and the overlap sits in the caption under it.
fn build_update_rate(app_controller: &Rc<RefCell<AppController>>) -> gtk::Box {
	let overlap = audio_config(&app_controller.borrow()).overlap;
	let caption_text = overlap_text(&app_controller.borrow(), overlap);

	let adjustment = gtk::Adjustment::new(
		overlap * 100.0,
		0.0,
		MAX_OVERLAP_PERCENT,
		OVERLAP_STEP_PERCENT,
		MAX_OVERLAP_PERCENT,
		0.0,
	);

	// Both labels read the window size and the sample rate as they stand, so
	// that a rate JACK changes under the stage reaches them.
	let controller = app_controller.clone();
	let slider = CaptionedSlider::new("Update rate", &caption_text, &adjustment, move |percent| {
		rate_label(&controller.borrow(), percent / 100.0)
	});

	let controller = app_controller.clone();
	let caption = slider.caption.clone();
	adjustment.connect_value_changed(move |adjustment| {
		caption.set_label(&overlap_text(
			&controller.borrow(),
			adjustment.value() / 100.0,
		));
	});

	let controller = app_controller.clone();
	slider
		.scale
		.connect_change_value(move |_scale, _, percent| {
			// The signal carries the value the scale is being moved to, which
			// the range has yet to clamp to its own bounds.
			let percent = percent.clamp(0.0, MAX_OVERLAP_PERCENT);
			set_overlap(&controller, percent / 100.0);
			glib::Propagation::Proceed
		});

	slider.widget
}

fn set_overlap(app_controller: &Rc<RefCell<AppController>>, overlap: f64) {
	let mut app_controller = app_controller.borrow_mut();
	let SpectrumGeneratorConfig::Audio(config) = &mut app_controller.config.spectrum_generator;
	config.overlap = overlap;
	handle_async_err(app_controller.update_spectrum_generator());
}

/// The tick rate in the words the stage row and the control both use.
pub fn rate_text(app_controller: &AppController) -> String {
	rate_label(app_controller, audio_config(app_controller).overlap)
}

/// The rate `overlap` produces, which is the rate the slider is reporting
/// while it is being dragged.
fn rate_label(app_controller: &AppController, overlap: f64) -> String {
	format!(
		"{:.0} / sec",
		sample_rate(app_controller) as f64 / hop(app_controller, overlap)
	)
}

/// What the rate costs in the DFT's own terms: how much of each window repeats
/// the last, and how much audio separates them.
fn overlap_text(app_controller: &AppController, overlap: f64) -> String {
	format!(
		"windows overlap {:.0}% · {:.0} ms hop",
		overlap * 100.0,
		1000.0 * hop(app_controller, overlap) / sample_rate(app_controller) as f64,
	)
}

fn hop(app_controller: &AppController, overlap: f64) -> f64 {
	audio::hop_samples(audio_config(app_controller).dft_window_size, overlap)
}

/// The grid the stage analyses on, in the terms its controls set.
fn grid_text(app_controller: &AppController) -> String {
	format!(
		"{} bins / octave · window {}",
		app_controller.config.samples_per_octave,
		audio_config(app_controller).dft_window_size,
	)
}

fn audio_config(app_controller: &AppController) -> &audio::Config {
	let SpectrumGeneratorConfig::Audio(config) = &app_controller.config.spectrum_generator;
	config
}

fn sample_rate(app_controller: &AppController) -> u32 {
	app_controller.sample_rate().unwrap_or(FALLBACK_SAMPLE_RATE)
}
