//! The spiral stage's body: the pitch range it draws, and the key it is
//! aligned to.

use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::GraphicGeneratorConfig;
use crate::controllers::AppController;
use crate::gui::controls::key_row::KeyRow;
use crate::gui::controls::range_slider::RangeSlider;
use crate::gui::controls::{CONTROL_SPACING, GROUP_SPACING, label_row};
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

pub fn new(app_controller: &Rc<RefCell<AppController>>) -> gtk::Box {
	let body = gtk::Box::new(gtk::Orientation::Vertical, GROUP_SPACING);
	body.add_css_class("stage-body");
	body.append(&build_pitch_range(app_controller));
	body.append(&build_key_row(app_controller).widget);
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

/// The note the spiral is keyed to.
pub fn key(app_controller: &AppController) -> Note {
	let GraphicGeneratorConfig::Spiral(config) = &app_controller.config.graphic_generator;
	Note::nearest(config.key_log_freq)
}
