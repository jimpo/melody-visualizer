//! The spectrum stage's body: the grid it analyses on, and how often it
//! produces a spectrum.

use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumGeneratorConfig;
use crate::controllers::AppController;
use crate::gui::controls::captioned_slider::CaptionedSlider;
use crate::gui::controls::{GROUP_SPACING, SEMITONES_PER_OCTAVE};
use crate::gui::handle_async_err;
use crate::spectrum::generators::audio;

/// The rate the labels read at while no source is open to report one.
const FALLBACK_SAMPLE_RATE: u32 = 48_000;

/// The coarsest grid the pitch resolution slider offers: one bin a semitone.
const MIN_DIVISIONS: f64 = 1.0;

/// The finest grid it offers, in bins per semitone. Twice the default, which
/// doubles the bin count and so quadruples the diffuser's cost — one step of
/// the cliff in DEVELOPMENT.md, and still three orders of magnitude inside a
/// tick.
const MAX_DIVISIONS: f64 = 30.0;

/// The most of a window the slider offers to repeat, as a percentage.
///
/// The hop is what is left of the window, so the rate the slider sets climbs
/// ever more steeply towards a whole window overlapping, for ever less of the
/// audio being new.
const MAX_OVERLAP_PERCENT: f64 = 75.0;

/// The step the slider moves the overlap in, as a percentage.
const OVERLAP_STEP_PERCENT: f64 = 5.0;

pub fn new(app_controller: &Rc<RefCell<AppController>>) -> gtk::Box {
	// The pitch resolution moves the window the update rate is derived from, so
	// its group is built second and handed the one it has to keep current.
	let update_rate = build_update_rate(app_controller);

	let body = gtk::Box::new(gtk::Orientation::Vertical, GROUP_SPACING);
	body.add_css_class("stage-body");
	body.append(&build_pitch_resolution(app_controller, &update_rate));
	body.append(&update_rate.widget);
	body
}

/// The pitch resolution group: how finely the grid divides a semitone, over a
/// slider that carries the DFT window with it.
///
/// The grid is what a listener hears apart and the window is what the DFT
/// needs to resolve it, so the division names the control and both figures sit
/// in the caption under it.
fn build_pitch_resolution(
	app_controller: &Rc<RefCell<AppController>>,
	update_rate: &CaptionedSlider,
) -> gtk::Box {
	let divisions = divisions(&app_controller.borrow());
	let caption_text = grid_text(&app_controller.borrow());

	let adjustment = gtk::Adjustment::new(
		divisions,
		MIN_DIVISIONS,
		MAX_DIVISIONS,
		1.0,
		MAX_DIVISIONS,
		0.0,
	);
	// The value label rounds the way the handler below does, so the two never
	// name different divisions either side of a half.
	let slider = CaptionedSlider::new(
		"Pitch resolution",
		&caption_text,
		&adjustment,
		|divisions| divisions_text(divisions.round()),
	);

	let controller = app_controller.clone();
	let caption = slider.caption.clone();
	let rate_value = update_rate.value.clone();
	let rate_caption = update_rate.caption.clone();
	slider
		.scale
		.connect_change_value(move |_scale, _, divisions| {
			// The signal carries the value the scale is being moved to, which
			// the range has yet to clamp to its own bounds, and which a jump
			// lands between two divisions.
			let divisions = divisions.clamp(MIN_DIVISIONS, MAX_DIVISIONS).round();
			let samples_per_octave = (divisions * SEMITONES_PER_OCTAVE) as usize;
			// A drag crosses many pixels per division, and regridding rebuilds
			// every cache hanging off the grid.
			if samples_per_octave == controller.borrow().config.samples_per_octave {
				return glib::Propagation::Proceed;
			}
			set_samples_per_octave(&controller, samples_per_octave);

			// Every label below reads the window, which the line above moved.
			let app_controller = controller.borrow();
			let overlap = audio_config(&app_controller).overlap;
			caption.set_label(&grid_text(&app_controller));
			rate_value.set_label(&rate_label(&app_controller, overlap));
			rate_caption.set_label(&overlap_text(&app_controller, overlap));
			glib::Propagation::Proceed
		});

	slider.widget
}

/// Puts the grid on `samples_per_octave` bins, and the DFT on the window that
/// resolves them.
///
/// Both threads pick the new grid up by comparing the `SpectrumParams` pointer,
/// so neither is told it changed beyond the config being shipped out.
fn set_samples_per_octave(app_controller: &Rc<RefCell<AppController>>, samples_per_octave: usize) {
	let mut app_controller = app_controller.borrow_mut();
	app_controller.config.samples_per_octave = samples_per_octave;
	let SpectrumGeneratorConfig::Audio(config) = &mut app_controller.config.spectrum_generator;
	config.dft_window_size = audio::dft_window_size(samples_per_octave);
	handle_async_err(app_controller.update_spectrum_params());
	handle_async_err(app_controller.update_spectrum_generator());
}

/// The bins the grid puts in a semitone.
pub fn divisions(app_controller: &AppController) -> f64 {
	app_controller.config.samples_per_octave as f64 / SEMITONES_PER_OCTAVE
}

/// The grid in the words the stage row and the control both use.
pub fn divisions_text(divisions: f64) -> String {
	format!("1/{divisions:.0} tone")
}

/// The update rate group: the rate the spectrum advances at, over a slider
/// that moves the overlap the rate comes from.
///
/// The rate is what a listener hears and the overlap is what the DFT does, so
/// the rate names the control and the overlap sits in the caption under it.
fn build_update_rate(app_controller: &Rc<RefCell<AppController>>) -> CaptionedSlider {
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

	slider
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

#[cfg(test)]
mod tests {
	use super::*;

	use crate::app::config::Config;

	#[test]
	fn the_default_grid_is_within_reach_of_the_slider() {
		let config = Config::default();
		let divisions = config.samples_per_octave as f64 / SEMITONES_PER_OCTAVE;
		// The adjustment clamps a value outside its range, and nothing writes
		// that back, so a default out of reach leaves the slider reporting a
		// grid the spectrum is not analysed on.
		assert!((MIN_DIVISIONS..=MAX_DIVISIONS).contains(&divisions));
		// The slider steps in whole divisions, so a default between two of them
		// is one the slider cannot return to.
		assert_eq!(divisions, divisions.round());
	}

	#[test]
	fn the_default_window_is_the_one_the_default_grid_asks_for() {
		let config = Config::default();
		let SpectrumGeneratorConfig::Audio(audio_config) = &config.spectrum_generator;
		assert_eq!(
			audio_config.dft_window_size,
			audio::dft_window_size(config.samples_per_octave),
			"the caption opens reading a window the pitch resolution would not set",
		);
	}
}
