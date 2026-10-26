//! A slider with two handles, for a range inside a range.
//!
//! GTK 4 has no two-handle scale, so this draws one on a `GtkDrawingArea`:
//! a rail across the widget, the fill between the handles, and a knob at
//! each. The handles are two `GtkAdjustment`s over the same bounds that
//! clamp each other, so the lower can never pass the upper. A drag moves the
//! handle nearest to where it began.

use gtk::prelude::*;
use std::cell::Cell;
use std::ops::RangeInclusive;
use std::rc::Rc;

/// The widget's height. The knobs are what set it.
const HEIGHT: i32 = 20;
const KNOB_RADIUS: f64 = 9.0;
const RAIL_HEIGHT: f64 = 4.0;

/// The rail and the knobs, in the pane's ink. The fill between the handles
/// is the widget's CSS `color`, which `style.css` sets to the accent.
const INK: (f64, f64, f64) = (1.0, 1.0, 1.0);
const RAIL_ALPHA: f64 = 0.15;
/// The knob's shadow: one pixel down, at this opacity.
const SHADOW_ALPHA: f64 = 0.35;

pub struct RangeSlider {
	/// The widget, to place in a control group.
	pub widget: gtk::DrawingArea,
	/// The lower handle. Its value never exceeds the upper's.
	pub lower: gtk::Adjustment,
	/// The upper handle. Its value is never under the lower's.
	pub upper: gtk::Adjustment,
}

/// Which handle a drag has hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handle {
	Lower,
	Upper,
}

impl RangeSlider {
	/// A slider over `bounds`, both handles moving in multiples of `step`
	/// from the start of it. The handles open at the ends of the bounds.
	pub fn new(bounds: RangeInclusive<f64>, step: f64) -> Self {
		let (start, end) = (*bounds.start(), *bounds.end());
		let lower = gtk::Adjustment::new(start, start, end, step, step, 0.0);
		let upper = gtk::Adjustment::new(end, start, end, step, step, 0.0);

		let widget = gtk::DrawingArea::builder()
			.content_height(HEIGHT)
			.hexpand(true)
			.build();
		widget.add_css_class("range-slider");

		let slider = RangeSlider {
			widget,
			lower,
			upper,
		};
		let geometry = Geometry { start, end, step };
		slider.connect_draw(geometry);
		slider.connect_drag(geometry);
		slider
	}

	/// Put the handles on `lower` and `upper`, each clamped into the bounds.
	pub fn set_values(&self, lower: f64, upper: f64) {
		self.lower.set_value(lower.min(upper));
		self.upper.set_value(upper.max(lower));
	}

	fn connect_draw(&self, geometry: Geometry) {
		let lower = self.lower.clone();
		let upper = self.upper.clone();
		self.widget
			.set_draw_func(move |widget, context, width, height| {
				let x_lower = geometry.knob_x(lower.value(), width as f64);
				let x_upper = geometry.knob_x(upper.value(), width as f64);
				draw(
					widget,
					context,
					width as f64,
					height as f64,
					x_lower,
					x_upper,
				);
			});

		for adjustment in [&self.lower, &self.upper] {
			let widget = self.widget.clone();
			adjustment.connect_value_changed(move |_adjustment| widget.queue_draw());
		}
	}

	fn connect_drag(&self, geometry: Geometry) {
		let drag = gtk::GestureDrag::new();
		let held = Rc::new(Cell::new(None::<Handle>));

		let lower = self.lower.clone();
		let upper = self.upper.clone();
		let held_clone = held.clone();
		drag.connect_drag_begin(move |gesture, x, _y| {
			let width = gesture.widget().map_or(0.0, |widget| widget.width() as f64);
			let handle = geometry.nearest_handle(x, width, lower.value(), upper.value());
			held_clone.set(Some(handle));
			move_handle(handle, geometry.value_at(x, width), &lower, &upper);
		});

		let lower = self.lower.clone();
		let upper = self.upper.clone();
		let held_clone = held.clone();
		drag.connect_drag_update(move |gesture, offset_x, _offset_y| {
			let (Some(handle), Some((start_x, _start_y))) =
				(held_clone.get(), gesture.start_point())
			else {
				return;
			};
			let width = gesture.widget().map_or(0.0, |widget| widget.width() as f64);
			move_handle(
				handle,
				geometry.value_at(start_x + offset_x, width),
				&lower,
				&upper,
			);
		});

		drag.connect_drag_end(move |_gesture, _offset_x, _offset_y| held.set(None));
		self.widget.add_controller(drag);
	}
}

/// Where the values sit along the widget: the bounds both handles move in,
/// and the step they move by.
#[derive(Clone, Copy)]
struct Geometry {
	start: f64,
	end: f64,
	step: f64,
}

impl Geometry {
	/// The x of the knob for `value`, in a widget `width` wide.
	///
	/// The rail is inset by a knob's radius at each end so the knobs stay
	/// inside the widget at the ends of the bounds.
	fn knob_x(&self, value: f64, width: f64) -> f64 {
		let fraction = if self.end > self.start {
			(value - self.start) / (self.end - self.start)
		} else {
			0.0
		};
		KNOB_RADIUS + fraction * (width - 2.0 * KNOB_RADIUS)
	}

	/// The value under `x`, on a step and inside the bounds.
	fn value_at(&self, x: f64, width: f64) -> f64 {
		let fraction = ((x - KNOB_RADIUS) / (width - 2.0 * KNOB_RADIUS)).clamp(0.0, 1.0);
		let value = self.start + fraction * (self.end - self.start);
		if self.step > 0.0 {
			self.start + ((value - self.start) / self.step).round() * self.step
		} else {
			value
		}
	}

	/// The handle a press at `x` takes hold of: the nearer one. When both are
	/// at the same spot, a press to the left takes the lower and one to the
	/// right takes the upper, so they can be pulled apart either way.
	fn nearest_handle(&self, x: f64, width: f64, lower: f64, upper: f64) -> Handle {
		let x_lower = self.knob_x(lower, width);
		let to_lower = (x - x_lower).abs();
		let to_upper = (x - self.knob_x(upper, width)).abs();
		if to_lower < to_upper || (to_lower == to_upper && x < x_lower) {
			Handle::Lower
		} else {
			Handle::Upper
		}
	}
}

/// Move `handle` to `value`, no further than the other handle.
fn move_handle(handle: Handle, value: f64, lower: &gtk::Adjustment, upper: &gtk::Adjustment) {
	let (new_lower, new_upper) = moved(handle, value, lower.value(), upper.value());
	lower.set_value(new_lower);
	upper.set_value(new_upper);
}

/// The handles after `handle` moves to `value`: it stops at the other one.
fn moved(handle: Handle, value: f64, lower: f64, upper: f64) -> (f64, f64) {
	match handle {
		Handle::Lower => (value.min(upper), upper),
		Handle::Upper => (lower, value.max(lower)),
	}
}

fn draw(
	widget: &gtk::DrawingArea,
	context: &cairo::Context,
	width: f64,
	height: f64,
	x_lower: f64,
	x_upper: f64,
) {
	let middle = height / 2.0;
	let (red, green, blue) = INK;

	// The rail, end to end.
	rounded_bar(context, KNOB_RADIUS, width - KNOB_RADIUS, middle);
	context.set_source_rgba(red, green, blue, RAIL_ALPHA);
	let _ = context.fill();

	// The fill between the handles.
	let accent = widget.color();
	rounded_bar(context, x_lower, x_upper, middle);
	context.set_source_rgba(
		accent.red() as f64,
		accent.green() as f64,
		accent.blue() as f64,
		accent.alpha() as f64,
	);
	let _ = context.fill();

	for x in [x_lower, x_upper] {
		context.arc(x, middle + 1.0, KNOB_RADIUS, 0.0, std::f64::consts::TAU);
		context.set_source_rgba(0.0, 0.0, 0.0, SHADOW_ALPHA);
		let _ = context.fill();
		context.arc(x, middle, KNOB_RADIUS, 0.0, std::f64::consts::TAU);
		context.set_source_rgb(red, green, blue);
		let _ = context.fill();
	}
}

/// A bar from `x_start` to `x_end` with round ends, centred on `y`.
fn rounded_bar(context: &cairo::Context, x_start: f64, x_end: f64, y: f64) {
	let radius = RAIL_HEIGHT / 2.0;
	context.new_path();
	context.arc(
		x_start,
		y,
		radius,
		0.5 * std::f64::consts::PI,
		1.5 * std::f64::consts::PI,
	);
	context.arc(
		x_end,
		y,
		radius,
		1.5 * std::f64::consts::PI,
		2.5 * std::f64::consts::PI,
	);
	context.close_path();
}

#[cfg(test)]
mod tests {
	use super::*;

	const GEOMETRY: Geometry = Geometry {
		start: 0.0,
		end: 10.0,
		step: 1.0,
	};
	const WIDTH: f64 = 100.0;

	#[test]
	fn the_knobs_sit_inside_the_widget_at_the_ends_of_the_bounds() {
		assert_eq!(GEOMETRY.knob_x(0.0, WIDTH), KNOB_RADIUS);
		assert_eq!(GEOMETRY.knob_x(10.0, WIDTH), WIDTH - KNOB_RADIUS);
	}

	#[test]
	fn a_value_lands_on_a_step_and_inside_the_bounds() {
		// A press at the widget's edge is past the rail, and reads as its end.
		assert_eq!(GEOMETRY.value_at(-5.0, WIDTH), 0.0);
		assert_eq!(GEOMETRY.value_at(200.0, WIDTH), 10.0);
		// A press between two steps rounds to the nearer.
		assert_eq!(GEOMETRY.value_at(GEOMETRY.knob_x(5.3, WIDTH), WIDTH), 5.0);
	}

	#[test]
	fn a_press_takes_the_nearer_handle() {
		let near_lower = GEOMETRY.knob_x(3.0, WIDTH);
		let near_upper = GEOMETRY.knob_x(7.0, WIDTH);
		assert_eq!(
			GEOMETRY.nearest_handle(near_lower, WIDTH, 2.0, 8.0),
			Handle::Lower
		);
		assert_eq!(
			GEOMETRY.nearest_handle(near_upper, WIDTH, 2.0, 8.0),
			Handle::Upper
		);
	}

	#[test]
	fn handles_on_the_same_spot_part_in_the_direction_of_the_press() {
		let x = GEOMETRY.knob_x(5.0, WIDTH);
		assert_eq!(
			GEOMETRY.nearest_handle(x - 1.0, WIDTH, 5.0, 5.0),
			Handle::Lower
		);
		assert_eq!(
			GEOMETRY.nearest_handle(x + 1.0, WIDTH, 5.0, 5.0),
			Handle::Upper
		);
	}

	#[test]
	fn the_handles_clamp_each_other() {
		// The lower handle dragged past the upper stops on it.
		assert_eq!(moved(Handle::Lower, 9.0, 2.0, 8.0), (8.0, 8.0));
		// And the upper handle cannot go under the lower.
		assert_eq!(moved(Handle::Upper, 1.0, 2.0, 8.0), (2.0, 2.0));
		// Short of each other, both move freely.
		assert_eq!(moved(Handle::Lower, 4.0, 2.0, 8.0), (4.0, 8.0));
		assert_eq!(moved(Handle::Upper, 6.0, 2.0, 8.0), (2.0, 6.0));
	}
}
