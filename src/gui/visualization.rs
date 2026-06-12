use gtk::prelude::*;
use std::{
	cell::RefCell,
	mem,
	rc::Rc,
};

use crate::controllers::VisualizationController;
use crate::error::Error;
use crate::graphic::Graphic;

pub fn new(controller: &Rc<RefCell<VisualizationController>>) -> impl IsA<gtk::Widget> {
	let drawing_area = gtk::DrawingArea::new();

	let drawing_area_clone = drawing_area.clone();
	let subscription = controller
		.borrow()
		.subscribe_graphic_updates(move || drawing_area_clone.queue_draw());

	let controller_clone = controller.clone();
	drawing_area.connect_draw(move |area, ctx| {
		if let Err(err) = on_draw(&mut controller_clone.borrow_mut(), area, ctx) {
			log::error!("error drawing to visualization pane: {}", err);
		}
		glib::Propagation::Proceed
	});

	drawing_area.connect_destroy(move |_| {
		let _ = &subscription;
	});

	drawing_area
}

fn on_draw(controller: &mut VisualizationController, area: &gtk::DrawingArea, ctx: &cairo::Context)
	-> Result<(), Error>
{
	let x_max = area.allocated_width();
	let y_max = area.allocated_height();
	let graphic = controller.graphic_mut();

	// Resize the graphic if it is the wrong size.
	if graphic.width() != x_max || graphic.height() != y_max {
		resize_surface(graphic, x_max, y_max)?;
	}

	graphic.with_image_surface(|surface| {
		ctx.set_source_surface(surface, 0f64, 0f64)?;
		ctx.paint()?;

		// Change the source, releasing the context's reference to the surface.
		ctx.set_source_rgb(0.0, 0.0, 0.0);

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
			ctx.fill()?;
			Ok(())
		})?;
	*graphic = new_graphic;
	Ok(())
}
