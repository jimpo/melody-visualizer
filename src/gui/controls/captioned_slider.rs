//! The captioned slider: a label with the value beside it, the scale under
//! them, and a dim caption saying what the control does.
//!
//! Every transform stage is one of these, so the pattern is built once here
//! and each stage supplies its adjustment and how to print its value.

use gtk::prelude::*;

use crate::gui::controls::{CONTROL_SPACING, caption, label_row};

pub struct CaptionedSlider {
	/// The whole group, to place in a stage body.
	pub widget: gtk::Box,
	/// The scale, for the stage to connect its change handler to.
	pub scale: gtk::Scale,
	/// The dim line under the scale, for a stage whose caption follows the
	/// value.
	pub caption: gtk::Label,
}

impl CaptionedSlider {
	/// Build the group over `adjustment`, with `format` printing its value for
	/// the label beside `label`.
	///
	/// The value label follows the adjustment, so it is current whichever way
	/// the value moves.
	pub fn new(
		label: &str,
		caption_text: &str,
		adjustment: &gtk::Adjustment,
		format: impl Fn(f64) -> String + 'static,
	) -> Self {
		let (row, value) = label_row(label);

		let scale = gtk::Scale::new(gtk::Orientation::Horizontal, Some(adjustment));
		scale.set_draw_value(false);
		scale.set_hexpand(true);

		value.set_label(&format(adjustment.value()));
		adjustment.connect_value_changed(move |adjustment| {
			value.set_label(&format(adjustment.value()));
		});

		let caption = caption(caption_text);

		let widget = gtk::Box::new(gtk::Orientation::Vertical, CONTROL_SPACING);
		widget.append(&row);
		widget.append(&scale);
		widget.append(&caption);

		CaptionedSlider {
			widget,
			scale,
			caption,
		}
	}
}
