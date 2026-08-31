use futures::prelude::*;
use gtk::prelude::*;
use std::{
	any::Any,
	cell::RefCell,
	mem,
	rc::{Rc, Weak},
	time::Duration,
};

use crate::async_processor::AsyncProcessor;
use crate::controllers::app::AppController;
use crate::error::Error;
use crate::graphic::renderer::GraphicRenderer;
use crate::graphic::{Graphic, GraphicBuffer};
use crate::pubsub::{Notifier, SubscriptionHandle};

const DEFAULT_FRAME_INTERVAL: u32 = 40;

enum RenderingState {
	Running,
	Idle(GraphicBuffer),
}

pub struct VisualizationController {
	app_controller: Rc<RefCell<AppController>>,
	notifier: Notifier,
	renderer: AsyncProcessor<GraphicRenderer>,
	graphic: Graphic,
	rendering_state: RenderingState,
	frame_interval_ms: u32,
}

impl VisualizationController {
	pub fn new(app_controller: Rc<RefCell<AppController>>) -> Rc<RefCell<Self>> {
		let renderer = app_controller.borrow().graphic_renderer().clone();
		let notifier = app_controller.borrow().pubsub().notifier();

		let controller = Rc::new(RefCell::new(VisualizationController {
			app_controller: app_controller.clone(),
			notifier,
			renderer,
			graphic: Graphic::default(),
			rendering_state: RenderingState::Idle(GraphicBuffer::default()),
			frame_interval_ms: DEFAULT_FRAME_INTERVAL,
		}));

		start_render_timer(&controller);

		controller
	}

	pub fn graphic(&self) -> &Graphic {
		&self.graphic
	}

	pub fn graphic_mut(&mut self) -> &mut Graphic {
		&mut self.graphic
	}

	pub fn subscribe_graphic_updates(&self, callback: impl Fn() + 'static) -> SubscriptionHandle {
		self.app_controller
			.borrow()
			.pubsub()
			.subscribe(move |_: &events::GraphicUpdate| callback())
	}

	pub fn set_frame_interval(&mut self, interval: Duration) {
		self.frame_interval_ms = interval.as_millis() as u32;
	}

	fn notify_and_log_err<T: Any + Send>(&self, notification: T) {
		if let Err(err) = self.notifier.send(notification) {
			log::error!("{}", Error::PubSub(err));
		}
	}
}

fn start_render_timer(controller: &Rc<RefCell<VisualizationController>>) {
	// Pass a weak ref into the timeout closure so that timeout doesn't keep controller alive
	// unnecessarily.
	let controller_ref = Rc::downgrade(controller);
	let old_frame_rate = controller.borrow().frame_interval_ms;
	glib::timeout_add_local(
		std::time::Duration::from_millis(old_frame_rate as u64),
		move || {
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
					glib::ControlFlow::Continue
				} else {
					start_render_timer(&controller);
					glib::ControlFlow::Break
				}
			} else {
				glib::ControlFlow::Break
			}
		},
	);
}

fn start_render(
	controller: &mut VisualizationController,
	controller_ref: Weak<RefCell<VisualizationController>>,
) -> bool {
	let rendering_state = mem::replace(&mut controller.rendering_state, RenderingState::Running);
	match rendering_state {
		RenderingState::Running => false,

		RenderingState::Idle(buffer) => {
			let graphic_fut = controller
				.renderer
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
							controller.notify_and_log_err(events::GraphicUpdate);
							old_graphic
						}
						Err(err) => {
							log::error!("error rendering frame: {}", err);
							controller.graphic.clone()
						}
					};
					assert!(matches!(
						controller.rendering_state,
						RenderingState::Running
					));
					controller.rendering_state = RenderingState::Idle(old_graphic.into_buffer());
				}
			});

			true
		}
	}
}

pub mod events {
	#[derive(Debug, Clone)]
	pub struct GraphicUpdate;
}
