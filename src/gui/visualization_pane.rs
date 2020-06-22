use std::cell::RefCell;
use std::rc::Rc;
use log::error;
use gtk::prelude::*;

use crate::application::Controller;
use std::convert::TryInto;

struct WidgetState {
	frames_since_last_buffer: usize,
}

pub struct VisualizationPane {
	state: Rc<RefCell<Controller>>,
	view: gtk::DrawingArea,
}

impl VisualizationPane {
	pub fn new(state: Rc<RefCell<Controller>>) -> Self {
		let area = gtk::DrawingArea::new();

		// let state_clone = state.clone();
		// area.connect_size_allocate(
		// 	move |area, alloc| on_size_allocate(&mut state_clone.borrow_mut(), area, alloc)
		// );

		let state_clone = state.clone();
		area.connect_draw(
			move |area, ctx| on_draw(&mut state_clone.borrow_mut(), area, ctx)
		);

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

	if let Some(ref surface) = graphic {
		ctx.set_source_surface(&**surface, 0f64, 0f64);
		ctx.fill();
	} else {
		ctx.set_source_rgb(0.0, 0.0, 0.0);
		ctx.rectangle(0.0, 0.0, x_max as f64, y_max as f64);
		ctx.fill();
	}

	let resize = graphic
		.as_ref()
		.map(|graphic| graphic.get_width() == x_max && graphic.get_height() == y_max)
		.unwrap_or(false);
	if resize {
		let new_surface = ctx
			.get_target()
			.create_similar_image(cairo::Format::Rgb24, x_max, y_max);
		match new_surface {
			Ok(new_surface) => {
				let new_surface = new_surface.try_into()
					.expect("create_similar_image must return an ImageSurface");
				*graphic = Some(new_surface);
			}
			Err(err) => error!("error creating new image surface: {}", err),
		}
	}

	Inhibit(false)
}

