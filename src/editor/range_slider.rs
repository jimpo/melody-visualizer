//! A slider with two handles, for a range inside a range: the spiral's pitch
//! range.
//!
//! iced has no two-handle slider, so this draws one on a canvas: a rail
//! across the widget, the fill between the handles, and a knob at each. A
//! handle is a parameter, and the two clamp each other, so the lower can
//! never pass the upper. A drag moves the handle nearest to where it began.

use nice_plug::prelude::*;
use nice_plug_iced::iced::{
	Color, Element, Length, Point, Rectangle, Renderer, Size, Theme, mouse,
	widget::{Canvas, canvas},
};

use super::Message;

/// The widget's height. The knobs are what set it.
const HEIGHT: f32 = 20.0;
const KNOB_RADIUS: f32 = 9.0;
const RAIL_HEIGHT: f32 = 4.0;
const RAIL: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.15);
/// The knob's shadow: one pixel down.
const SHADOW: Color = Color::from_rgba(0.0, 0.0, 0.0, 0.35);

/// A slider over two parameters with the same range, `lower` never above
/// `upper`.
pub fn range_slider<'a>(lower: &'a IntParam, upper: &'a IntParam) -> Element<'a, Message> {
	Canvas::new(RangeSlider { lower, upper })
		.width(Length::Fill)
		.height(HEIGHT)
		.into()
}

struct RangeSlider<'a> {
	lower: &'a IntParam,
	upper: &'a IntParam,
}

/// Which handle a drag has hold of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Handle {
	Lower,
	Upper,
}

impl RangeSlider<'_> {
	/// Both handles, where the parameters sit in their normalized range.
	fn handles(&self) -> (f32, f32) {
		(
			self.lower.unmodulated_normalized_value(),
			self.upper.unmodulated_normalized_value(),
		)
	}

	/// The message that moves `handle` to `x`, on the parameter's step and no
	/// further than the other handle.
	fn drag(&self, handle: Handle, x: f32, width: f32) -> Message {
		let param = match handle {
			Handle::Lower => self.lower,
			Handle::Upper => self.upper,
		};
		let snapped = param.preview_normalized(param.preview_plain(fraction_at(x, width)));
		let (lower, upper) = self.handles();
		let (lower, upper) = moved(handle, snapped, lower, upper);
		match handle {
			Handle::Lower => Message::Drag(self.lower.as_ptr(), lower),
			Handle::Upper => Message::Drag(self.upper.as_ptr(), upper),
		}
	}
}

impl canvas::Program<Message> for RangeSlider<'_> {
	type State = Option<Handle>;

	fn update(
		&self,
		dragging: &mut Option<Handle>,
		event: &canvas::Event,
		bounds: Rectangle,
		cursor: mouse::Cursor,
	) -> Option<canvas::Action<Message>> {
		let canvas::Event::Mouse(event) = event else {
			return None;
		};
		match event {
			mouse::Event::ButtonPressed(mouse::Button::Left) => {
				let position = cursor.position_in(bounds)?;
				let (lower, upper) = self.handles();
				let handle = nearest_handle(position.x, bounds.width, lower, upper);
				*dragging = Some(handle);
				Some(
					canvas::Action::publish(self.drag(handle, position.x, bounds.width))
						.and_capture(),
				)
			}
			mouse::Event::CursorMoved { position } => {
				let handle = (*dragging)?;
				let x = position.x - bounds.x;
				Some(canvas::Action::publish(self.drag(handle, x, bounds.width)).and_capture())
			}
			mouse::Event::ButtonReleased(mouse::Button::Left) => {
				dragging.take()?;
				Some(canvas::Action::publish(Message::Release).and_capture())
			}
			_ => None,
		}
	}

	fn draw(
		&self,
		_dragging: &Option<Handle>,
		renderer: &Renderer,
		theme: &Theme,
		bounds: Rectangle,
		_cursor: mouse::Cursor,
	) -> Vec<canvas::Geometry> {
		let mut frame = canvas::Frame::new(renderer, bounds.size());
		let width = bounds.width;
		let middle = bounds.height / 2.0;
		let (lower, upper) = self.handles();
		let (x_lower, x_upper) = (knob_x(lower, width), knob_x(upper, width));

		let bar = |frame: &mut canvas::Frame, start: f32, end: f32, colour: Color| {
			let path = canvas::Path::rounded_rectangle(
				Point::new(start - RAIL_HEIGHT / 2.0, middle - RAIL_HEIGHT / 2.0),
				Size::new(end - start + RAIL_HEIGHT, RAIL_HEIGHT),
				(RAIL_HEIGHT / 2.0).into(),
			);
			frame.fill(&path, colour);
		};
		bar(&mut frame, KNOB_RADIUS, width - KNOB_RADIUS, RAIL);
		bar(&mut frame, x_lower, x_upper, theme.palette().primary);

		for x in [x_lower, x_upper] {
			frame.fill(
				&canvas::Path::circle(Point::new(x, middle + 1.0), KNOB_RADIUS),
				SHADOW,
			);
			frame.fill(
				&canvas::Path::circle(Point::new(x, middle), KNOB_RADIUS),
				Color::WHITE,
			);
		}
		vec![frame.into_geometry()]
	}

	fn mouse_interaction(
		&self,
		dragging: &Option<Handle>,
		bounds: Rectangle,
		cursor: mouse::Cursor,
	) -> mouse::Interaction {
		if dragging.is_some() {
			mouse::Interaction::Grabbing
		} else if cursor.is_over(bounds) {
			mouse::Interaction::Pointer
		} else {
			mouse::Interaction::default()
		}
	}
}

/// The x of the knob at `fraction` of the range, in a widget `width` wide.
///
/// The rail is inset by a knob's radius at each end so the knobs stay inside
/// the widget at the ends of the range.
fn knob_x(fraction: f32, width: f32) -> f32 {
	KNOB_RADIUS + fraction * (width - 2.0 * KNOB_RADIUS)
}

/// The fraction of the range under `x`, inside the range.
fn fraction_at(x: f32, width: f32) -> f32 {
	((x - KNOB_RADIUS) / (width - 2.0 * KNOB_RADIUS)).clamp(0.0, 1.0)
}

/// The handle a press at `x` takes hold of: the nearer one. When both are at
/// the same spot, a press to the left takes the lower and one to the right
/// takes the upper, so they can be pulled apart either way.
fn nearest_handle(x: f32, width: f32, lower: f32, upper: f32) -> Handle {
	let x_lower = knob_x(lower, width);
	let to_lower = (x - x_lower).abs();
	let to_upper = (x - knob_x(upper, width)).abs();
	if to_lower < to_upper || (to_lower == to_upper && x < x_lower) {
		Handle::Lower
	} else {
		Handle::Upper
	}
}

/// The handles after `handle` moves to `value`: it stops at the other one.
fn moved(handle: Handle, value: f32, lower: f32, upper: f32) -> (f32, f32) {
	match handle {
		Handle::Lower => (value.min(upper), upper),
		Handle::Upper => (lower, value.max(lower)),
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const WIDTH: f32 = 100.0;

	#[test]
	fn the_knobs_sit_inside_the_widget_at_the_ends_of_the_range() {
		assert_eq!(knob_x(0.0, WIDTH), KNOB_RADIUS);
		assert_eq!(knob_x(1.0, WIDTH), WIDTH - KNOB_RADIUS);
		// A press past either end of the rail reads as that end.
		assert_eq!(fraction_at(-5.0, WIDTH), 0.0);
		assert_eq!(fraction_at(200.0, WIDTH), 1.0);
	}

	#[test]
	fn a_press_takes_the_nearer_handle() {
		assert_eq!(
			nearest_handle(knob_x(0.3, WIDTH), WIDTH, 0.2, 0.8),
			Handle::Lower
		);
		assert_eq!(
			nearest_handle(knob_x(0.7, WIDTH), WIDTH, 0.2, 0.8),
			Handle::Upper
		);
	}

	#[test]
	fn handles_on_the_same_spot_part_in_the_direction_of_the_press() {
		let x = knob_x(0.5, WIDTH);
		assert_eq!(nearest_handle(x - 1.0, WIDTH, 0.5, 0.5), Handle::Lower);
		assert_eq!(nearest_handle(x + 1.0, WIDTH, 0.5, 0.5), Handle::Upper);
	}

	#[test]
	fn the_handles_clamp_each_other() {
		// The lower handle dragged past the upper stops on it.
		assert_eq!(moved(Handle::Lower, 0.9, 0.2, 0.8), (0.8, 0.8));
		// And the upper handle cannot go under the lower.
		assert_eq!(moved(Handle::Upper, 0.1, 0.2, 0.8), (0.2, 0.2));
		// Short of each other, both move freely.
		assert_eq!(moved(Handle::Lower, 0.4, 0.2, 0.8), (0.4, 0.8));
		assert_eq!(moved(Handle::Upper, 0.6, 0.2, 0.8), (0.2, 0.6));
	}
}
