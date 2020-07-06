use std::cell::RefCell;
use std::rc::Rc;
use log::error;
use gtk::prelude::*;

use crate::application::Controller;
use std::convert::TryInto;
use std::borrow::Borrow;

struct WidgetState {
	frames_since_last_buffer: usize,
}

pub struct VisualizationPane {
	state: Rc<RefCell<Controller>>,
	view: Rc<gtk::DrawingArea>,
}

impl VisualizationPane {
	pub fn new(controller: Rc<RefCell<Controller>>) -> Self {
		let area = Rc::new(gtk::DrawingArea::new());

		// let state_clone = state.clone();
		// area.connect_size_allocate(
		// 	move |area, alloc| on_size_allocate(&mut state_clone.borrow_mut(), area, alloc)
		// );

		let controller_clone = controller.clone();
		area.connect_draw(
			move |area, ctx| on_draw(&mut controller_clone.borrow_mut(), area, ctx)
		);

		controller.borrow_mut()
			.subscribe_graphic_update(|| area.queue_draw());

		VisualizationPane {
			state,
			view: area,
		}
	}

	pub fn widget(&self) -> &gtk::DrawingArea {
		&self.view
	}
}

fn on_draw(state: &mut Controller, area: &gtk::DrawingArea, ctx: &cairo::Context) -> Inhibit {
	let x_max = area.get_allocated_width();
	let y_max = area.get_allocated_height();
	let graphic = state.graphic_mut();

	// Resize the graphic if it is the wrong size.
	if graphic.get_width() != x_max || graphic.get_height() != y_max {
		match resize_surface(ctx, graphic, x_max, y_max) {
			Some(new_surface) => *graphic = new_surface,
			Err(err) => {
				error!("error creating new image surface: {}", err);
				return Inhibit(false);
			}
		}
	}

	ctx.set_source_surface(&**graphic, 0f64, 0f64);
	ctx.fill();

	Inhibit(false)
}

fn resize_surface(ctx: &cairo::Context, graphic: &cairo::ImageSurface, x_max: i32, y_max: i32)
	-> Result<cairo::ImageSurface, cairo::Error>
{
	let new_surface = ctx
		.get_target()
		.create_similar_image(cairo::Format::Rgb24, x_max, y_max)?
		.try_into()
		.expect("create_similar_image must return an ImageSurface");

	// Set the new surface to all black.
	// TODO: Attempt to modify the old surface maybe?
	let new_ctx = cairo::Content::new(&new_surface);
	new_ctx.set_source_rgb(0.0, 0.0, 0.0);
	new_ctx.rectangle(0.0, 0.0, x_max as f64, y_max as f64);
	new_ctx.fill();

	Ok(new_surface)
}
