//! The spiral stage's body: the pitch range it draws, the key it is aligned
//! to, the ring of interval names, and the two paddings that fix its annulus
//! on the surface.

use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::GraphicGeneratorConfig;
use crate::controllers::AppController;
use crate::graphic::generators::spiral;
use crate::gui::controls::captioned_slider::CaptionedSlider;
use crate::gui::controls::key_row::KeyRow;
use crate::gui::controls::range_slider::RangeSlider;
use crate::gui::controls::{CONTROL_SPACING, GROUP_SPACING, caption, label_row};
use crate::gui::handle_async_err;
use crate::note;
use crate::note::Note;

/// The ends of the pitch range slider: the bottom and top of a piano.
const MIN_NOTE: Note = note!(A, 0);
const MAX_NOTE: Note = note!(C, 8);

/// A semitone, in the log₂(Hz) the range moves in.
const SEMITONE: f64 = 1.0 / 12.0;

/// The octave the key is set in. Only its pitch class shows: every octave of
/// the key sits on the same spoke of the spiral.
const KEY_OCTAVE: i8 = 4;

/// The widest centre hole the slider offers, in pixels.
const MAX_CENTER_PAD: f64 = 200.0;

/// The widest outer margin the slider offers, in pixels.
const MAX_OUTER_PAD: f64 = 100.0;

pub fn new(app_controller: &Rc<RefCell<AppController>>) -> gtk::Box {
	let body = gtk::Box::new(gtk::Orientation::Vertical, GROUP_SPACING);
	body.add_css_class("stage-body");
	body.append(&build_pitch_range(app_controller));
	body.append(&build_key_row(app_controller).widget);
	body.append(&build_interval_ring(app_controller));
	body.append(&build_pad(
		app_controller,
		"Centre hole",
		"The empty disc the lowest ring is drawn around.",
		MAX_CENTER_PAD,
		|config| config.center_pad,
		|config, pixels| config.center_pad = pixels,
	));
	body.append(&build_pad(
		app_controller,
		"Outer padding",
		"The margin between the highest ring and the nearer edge.",
		MAX_OUTER_PAD,
		|config| config.outer_pad,
		|config, pixels| config.outer_pad = pixels,
	));
	body
}

/// The pitch range group: the label with the range in notes beside it, the
/// two-handle slider, and the ends of the range in hertz under it.
fn build_pitch_range(app_controller: &Rc<RefCell<AppController>>) -> gtk::Box {
	let (row, value) = label_row("Pitch range");

	let slider = RangeSlider::new(
		MIN_NOTE.log_frequency()..=MAX_NOTE.log_frequency(),
		SEMITONE,
	);
	{
		let config = &app_controller.borrow().config;
		slider.set_values(config.min_freq.log2(), config.max_freq.log2());
	}

	let min_hz = hz_caption(0.0);
	min_hz.set_hexpand(true);
	let max_hz = hz_caption(1.0);
	let captions = gtk::Box::new(gtk::Orientation::Horizontal, 0);
	captions.append(&min_hz);
	captions.append(&max_hz);

	let show = {
		let lower = slider.lower.clone();
		let upper = slider.upper.clone();
		move || {
			value.set_label(&format!(
				"{} – {}",
				Note::nearest(lower.value()),
				Note::nearest(upper.value())
			));
			min_hz.set_label(&format!("{:.0} Hz", lower.value().exp2()));
			max_hz.set_label(&format!("{:.0} Hz", upper.value().exp2()));
		}
	};
	show();

	// The handlers go on after the slider is set from the config, so that the
	// setting is not sent straight back.
	let show = Rc::new(show);
	let show_clone = show.clone();
	let app_controller_clone = app_controller.clone();
	slider.lower.connect_value_changed(move |lower| {
		show_clone();
		set_min_freq(&app_controller_clone, lower.value().exp2());
	});
	let app_controller_clone = app_controller.clone();
	slider.upper.connect_value_changed(move |upper| {
		show();
		set_max_freq(&app_controller_clone, upper.value().exp2());
	});

	let group = gtk::Box::new(gtk::Orientation::Vertical, CONTROL_SPACING);
	group.append(&row);
	group.append(&slider.widget);
	group.append(&captions);
	group
}

fn hz_caption(xalign: f32) -> gtk::Label {
	let label = gtk::Label::builder().xalign(xalign).build();
	label.add_css_class("caption");
	label
}

fn set_min_freq(app_controller: &Rc<RefCell<AppController>>, freq: f64) {
	let mut app_controller = app_controller.borrow_mut();
	app_controller.config.min_freq = freq;
	handle_async_err(app_controller.update_spectrum_params());
}

fn set_max_freq(app_controller: &Rc<RefCell<AppController>>, freq: f64) {
	let mut app_controller = app_controller.borrow_mut();
	app_controller.config.max_freq = freq;
	handle_async_err(app_controller.update_spectrum_params());
}

/// The key row, down on the key the spiral is set to and moving it after that.
fn build_key_row(app_controller: &Rc<RefCell<AppController>>) -> KeyRow {
	let key_row = KeyRow::new("Key", "Sits at the top of the wheel and anchors the hue.");
	key_row.set_selected(key(&app_controller.borrow()).pitch_class);

	let app_controller = app_controller.clone();
	key_row.connect_selected(move |pitch_class| {
		let async_update = {
			let mut app_controller = app_controller.borrow_mut();
			let GraphicGeneratorConfig::Spiral(config) =
				&mut app_controller.config.graphic_generator;
			config.key_log_freq = Note {
				octave: KEY_OCTAVE,
				pitch_class,
			}
			.log_frequency();
			app_controller.update_graphic_generator()
		};
		handle_async_err(async_update);
	});
	key_row
}

/// One of the two padding sliders, in pixels, reading its field through `get`
/// and writing it through `set`.
///
/// Both reconfigure the generator in place, so a drag reshapes the spiral
/// rather than rebuilding it.
fn build_pad(
	app_controller: &Rc<RefCell<AppController>>,
	label: &str,
	caption: &str,
	max: f64,
	get: fn(&spiral::Config) -> f64,
	set: fn(&mut spiral::Config, f64),
) -> gtk::Box {
	let initial = {
		let app_controller = app_controller.borrow();
		let GraphicGeneratorConfig::Spiral(config) = &app_controller.config.graphic_generator;
		get(config)
	};

	let adjustment = gtk::Adjustment::new(initial, 0.0, max, 1.0, max / 4.0, 0.0);
	let slider = CaptionedSlider::new(label, caption, &adjustment, |pixels| {
		format!("{pixels:.0} px")
	});

	let app_controller = app_controller.clone();
	slider.scale.connect_change_value(move |_scale, _, value| {
		// A jump to a position outside the trough reports a value outside the
		// range, and a negative padding turns the annulus inside out.
		let pixels = value.clamp(0.0, max);
		let async_update = {
			let mut app_controller = app_controller.borrow_mut();
			let GraphicGeneratorConfig::Spiral(config) =
				&mut app_controller.config.graphic_generator;
			set(config, pixels);
			app_controller.update_graphic_generator()
		};
		handle_async_err(async_update);
		glib::Propagation::Proceed
	});

	slider.widget
}

/// The switch that shows or hides the ring of interval names round the spiral.
fn build_interval_ring(app_controller: &Rc<RefCell<AppController>>) -> gtk::Box {
	let (row, _value) = label_row("Interval ring");
	let active = {
		let app_controller = app_controller.borrow();
		let GraphicGeneratorConfig::Spiral(config) = &app_controller.config.graphic_generator;
		config.interval_ring
	};
	let switch = gtk::Switch::builder()
		.active(active)
		.valign(gtk::Align::Center)
		.build();
	row.append(&switch);

	let app_controller = app_controller.clone();
	switch.connect_active_notify(move |switch| {
		let async_update = {
			let mut app_controller = app_controller.borrow_mut();
			let GraphicGeneratorConfig::Spiral(config) =
				&mut app_controller.config.graphic_generator;
			config.interval_ring = switch.is_active();
			app_controller.update_graphic_generator()
		};
		handle_async_err(async_update);
	});

	let group = gtk::Box::new(gtk::Orientation::Vertical, CONTROL_SPACING);
	group.append(&row);
	group.append(&caption(&format!(
		"Names each spoke by its interval from the key. Takes {:.0} px from the spiral.",
		spiral::RING_ROOM,
	)));
	group
}

/// The note the spiral is keyed to.
pub fn key(app_controller: &AppController) -> Note {
	let GraphicGeneratorConfig::Spiral(config) = &app_controller.config.graphic_generator;
	Note::nearest(config.key_log_freq)
}

#[cfg(test)]
mod tests {
	use super::*;

	use crate::app::config::Config;

	#[test]
	fn the_default_paddings_are_within_reach_of_their_sliders() {
		let GraphicGeneratorConfig::Spiral(config) = Config::default().graphic_generator;
		// The adjustment clamps a value outside its range, and nothing writes
		// that back, so a default out of reach leaves the slider and the label
		// reporting a padding the spiral is not drawn with.
		assert!((0.0..=MAX_CENTER_PAD).contains(&config.center_pad));
		assert!((0.0..=MAX_OUTER_PAD).contains(&config.outer_pad));
	}
}
