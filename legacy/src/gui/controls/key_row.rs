//! The key row: twelve buttons, one per pitch class, of which one is down.
//!
//! The row is a group of toggle buttons, so pressing one releases the one
//! that was down. The black keys are drawn darker, as on a keyboard.

use gtk::prelude::*;
use std::rc::Rc;

use crate::gui::controls::{CONTROL_SPACING, caption};
use crate::note::PitchClass;

/// Every pitch class in the order it sits on the row, and how it is drawn:
/// its name, and whether it is a black key.
const KEYS: [(PitchClass, &str, bool); 12] = [
	(PitchClass::C, "C", false),
	(PitchClass::Db, "C♯", true),
	(PitchClass::D, "D", false),
	(PitchClass::Eb, "E♭", true),
	(PitchClass::E, "E", false),
	(PitchClass::F, "F", false),
	(PitchClass::Gb, "F♯", true),
	(PitchClass::G, "G", false),
	(PitchClass::Ab, "A♭", true),
	(PitchClass::A, "A", false),
	(PitchClass::Bb, "B♭", true),
	(PitchClass::B, "B", false),
];

/// The gap between two keys.
const KEY_SPACING: i32 = 2;

/// The name of a pitch class as the key row prints it, so a stage row that
/// names the key agrees with the button that is down.
pub fn key_name(pitch_class: PitchClass) -> &'static str {
	KEYS.iter()
		.find(|(key, _name, _black)| *key == pitch_class)
		.map(|(_key, name, _black)| *name)
		.expect("KEYS holds every pitch class")
}

pub struct KeyRow {
	/// The whole group, to place in a stage body.
	pub widget: gtk::Box,
	buttons: Vec<(PitchClass, gtk::ToggleButton)>,
}

impl KeyRow {
	/// Build the group under `label`, with no key down yet.
	pub fn new(label: &str, caption_text: &str) -> Self {
		let label = gtk::Label::builder().label(label).xalign(0.0).build();
		label.add_css_class("control-label");

		let row = gtk::Box::builder()
			.orientation(gtk::Orientation::Horizontal)
			.spacing(KEY_SPACING)
			.homogeneous(true)
			.build();
		row.add_css_class("key-row");

		let mut buttons = Vec::with_capacity(KEYS.len());
		for (pitch_class, name, black) in KEYS {
			let button = gtk::ToggleButton::with_label(name);
			if black {
				button.add_css_class("black-key");
			}
			if let Some((_first, group)) = buttons.first() {
				button.set_group(Some(group));
			}
			row.append(&button);
			buttons.push((pitch_class, button));
		}

		let widget = gtk::Box::new(gtk::Orientation::Vertical, CONTROL_SPACING);
		widget.append(&label);
		widget.append(&row);
		widget.append(&caption(caption_text));

		KeyRow { widget, buttons }
	}

	/// Put the key of `pitch_class` down.
	pub fn set_selected(&self, pitch_class: PitchClass) {
		for (key, button) in &self.buttons {
			if *key == pitch_class {
				button.set_active(true);
			}
		}
	}

	/// Call `handler` with the pitch class of each key as it goes down.
	pub fn connect_selected(&self, handler: impl Fn(PitchClass) + 'static) {
		let handler = Rc::new(handler);
		for (pitch_class, button) in &self.buttons {
			let pitch_class = *pitch_class;
			let handler = handler.clone();
			button.connect_toggled(move |button| {
				if button.is_active() {
					handler(pitch_class);
				}
			});
		}
	}
}
