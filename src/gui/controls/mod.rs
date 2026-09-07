//! The bodies of the stages, and the widget vocabulary they are built from.
//!
//! A transform stage is one captioned slider; the spectrum and spiral stages
//! combine several groups. The helpers here carry the spacing and the
//! label row every group shares.

pub mod captioned_slider;
pub mod decibel_converter;
pub mod diffuser;
pub mod key_row;
pub mod range_slider;
pub mod spectrum;
pub mod spiral;
pub mod volume_normalizer;

use gtk::prelude::*;

/// The gap between two control groups in one stage body.
pub const GROUP_SPACING: i32 = 22;

/// The gap between the parts of one control group: its label row, the control
/// itself, and the caption under it.
pub const CONTROL_SPACING: i32 = 8;

/// The row over a control: its label on the left, its current value on the
/// right.
///
/// Returns the row and the value label, for the caller to keep current.
pub fn label_row(label: &str) -> (gtk::Box, gtk::Label) {
	let label = gtk::Label::builder()
		.label(label)
		.xalign(0.0)
		.hexpand(true)
		.build();
	label.add_css_class("control-label");

	let value = gtk::Label::builder().xalign(1.0).build();
	value.add_css_class("control-value");

	let row = gtk::Box::new(gtk::Orientation::Horizontal, 0);
	row.append(&label);
	row.append(&value);
	(row, value)
}

/// A dim line under a control saying what it does.
pub fn caption(text: &str) -> gtk::Label {
	let caption = gtk::Label::builder()
		.label(text)
		.xalign(0.0)
		.wrap(true)
		.build();
	caption.add_css_class("caption");
	caption
}
