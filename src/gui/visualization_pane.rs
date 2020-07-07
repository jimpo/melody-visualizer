use gtk::prelude::*;
use log::{debug, error};
use std::borrow::Borrow;
use std::cell::RefCell;
use std::convert::TryInto;
use std::mem;
use std::rc::Rc;

use crate::application::Controller;
use crate::error::Error;
use crate::graphic_renderer::{Graphic, GraphicBuffer};

struct WidgetState {
	frames_since_last_buffer: usize,
}

pub struct VisualizationPane {
	controller: Rc<RefCell<Controller>>,
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
		area.connect_draw(move |area, ctx| {
			if let Err(err) = on_draw(&mut controller_clone.borrow_mut(), area, ctx) {
				error!("error drawing to visualization pane: {}", err);
			}
			Inhibit(false)
		});

		let area_clone = area.clone();
		controller.borrow_mut()
			.subscribe_graphic_update(move || area_clone.queue_draw());

		VisualizationPane {
			controller,
			view: area,
		}
	}

	pub fn widget(&self) -> &gtk::DrawingArea {
		&self.view
	}
}

fn on_draw(state: &mut Controller, area: &gtk::DrawingArea, ctx: &cairo::Context)
	-> Result<(), Error>
{
	debug!("redrawing visualization pane");

	let x_max = area.get_allocated_width();
	let y_max = area.get_allocated_height();
	let graphic = &mut *state.graphic_mut();

	// Resize the graphic if it is the wrong size.
	if graphic.width() != x_max || graphic.height() != y_max {
		resize_surface(graphic, x_max, y_max)?;
	}

	graphic.with_image_surface(|surface| {
		ctx.set_source_surface(surface, 0f64, 0f64);
		ctx.rectangle(0.0, 0.0, x_max as f64, y_max as f64);
		ctx.fill();
		Ok(())
	})
}

fn resize_surface(graphic: &mut Graphic, x_max: i32, y_max: i32) -> Result<(), Error> {
	let old_graphic = mem::replace(graphic, Graphic::default());
	let new_graphic = old_graphic
		.into_buffer()
		.resize(x_max, y_max)
		.draw(|ctx| {
			// Set the new surface to all black.
			// TODO: Attempt to modify the old surface maybe?
			ctx.set_source_rgb(0.0, 0.0, 0.0);
			ctx.rectangle(0.0, 0.0, x_max as f64, y_max as f64);
			ctx.fill();
			Ok(())
		})?;
	*graphic = new_graphic;
	Ok(())
}
