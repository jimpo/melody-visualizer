use futures::{prelude::*, future::LocalBoxFuture};
use gtk::prelude::*;
use std::{
	cell::RefCell,
	mem,
	rc::{Rc, Weak},
	time::Duration,
};

use crate::application::{events::GraphicUpdate, Controller as AppController};
use crate::async_processor::AsyncProcessor;
use crate::gui::error_dialog;
use crate::error::Error;
use crate::graphic::{Graphic, GraphicBuffer};
use crate::graphic_renderer::GraphicRenderer;

const DEFAULT_FRAME_INTERVAL: u32 = 40;

enum RenderingState {
	Running,
	Idle(GraphicBuffer),
}

pub struct Controller {
	app_controller: Rc<RefCell<AppController>>,
	graphic: Graphic,
	renderer: AsyncProcessor<GraphicRenderer>,
	rendinging_state: RenderingState,
	frame_interval_ms: u32,
	drawing_area: gtk::DrawingArea,
}

impl Controller {
	pub fn set_frame_interval(&mut self, interval: Duration) {
		self.frame_interval_ms = interval.as_millis() as u32;
	}
}

pub fn new(app_controller: Rc<RefCell<AppController>>)
	-> (Rc<RefCell<Controller>>, gtk::DrawingArea)
{
	let drawing_area = gtk::DrawingArea::new();
	let renderer = app_controller.borrow().graphic_renderer().clone();

	let controller = Rc::new(RefCell::new(Controller {
		app_controller: app_controller.clone(),
		graphic: Graphic::default(),
		rendinging_state: RenderingState::Idle(GraphicBuffer::default()),
		renderer,
		frame_interval_ms: DEFAULT_FRAME_INTERVAL,
		drawing_area: drawing_area.clone(),
	}));

	let controller_clone = controller.clone();
	drawing_area.connect_draw(move |area, ctx| {
		if let Err(err) = on_draw(&mut controller_clone.borrow_mut(), area, ctx) {
			log::error!("error drawing to visualization pane: {}", err);
		}
		Inhibit(false)
	});

	start_render_timer(&controller);

	(controller, drawing_area)
}

fn on_draw(controller: &mut Controller, area: &gtk::DrawingArea, ctx: &cairo::Context)
	-> Result<(), Error>
{
	let x_max = area.get_allocated_width();
	let y_max = area.get_allocated_height();
	let graphic = &mut controller.graphic;

	// Resize the graphic if it is the wrong size.
	if graphic.width() != x_max || graphic.height() != y_max {
		resize_surface(graphic, x_max, y_max)?;
	}

	graphic.with_image_surface(|surface| {
		ctx.set_source_surface(surface, 0f64, 0f64);
		ctx.paint();

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
			ctx.fill();
			Ok(())
		})?;
	*graphic = new_graphic;
	Ok(())
}

fn start_render_timer(controller: &Rc<RefCell<Controller>>) {
	// Pass a weak ref into the timeout closure so that timeout doesn't keep controller alive
	// unnecessarily.
	let controller_ref = Rc::downgrade(controller);
	let old_frame_rate = controller.borrow().frame_interval_ms;
	gtk::timeout_add(old_frame_rate, move || {
		if let Some(controller) = controller_ref.clone().upgrade() {
			let new_frame_rate;
			{
				let mut controller = controller.borrow_mut();
				new_frame_rate = controller.frame_interval_ms;
				if !start_render(&mut *controller, controller_ref.clone()) {
					log::debug!("skipping frame because last frame is still rendering");
				}
			};

			// If frame rate has changed, start a new timer.
			if old_frame_rate == new_frame_rate {
				Continue(true)
			} else {
				start_render_timer(&controller);
				Continue(false)
			}
		} else {
			Continue(false)
		}
	});
}

fn start_render(controller: &mut Controller, controller_ref: Weak<RefCell<Controller>>) -> bool {
	let rendering_state = mem::replace(
		&mut controller.rendinging_state,
		RenderingState::Running
	);
	match rendering_state {
		RenderingState::Running => false,

		RenderingState::Idle(buffer) => {
			let graphic_fut = controller.renderer
				.exec_cloned(move |renderer| renderer.render(buffer))
				.map(|result| {
					result
						.map_err(Error::Communication)
						.and_then(|result| result)
				});

			glib::MainContext::default().spawn_local(async move {
				let result = graphic_fut.await;
				if let Some(controller) = controller_ref.upgrade() {
					let mut controller = controller.borrow_mut();
					let old_graphic = match result {
						Ok(graphic) => {
							let old_graphic = mem::replace(&mut controller.graphic, graphic);
							controller.drawing_area.queue_draw();
							old_graphic
						}
						Err(err) => {
							log::error!("error rendering frame: {}", err);
							controller.graphic.clone()
						}
					};
					assert!(matches!(controller.rendinging_state, RenderingState::Running));
					controller.rendinging_state = RenderingState::Idle(old_graphic.into_buffer());
				}
			});

			true
		}
	}
}
