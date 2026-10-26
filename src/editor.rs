//! The plugin's window: the spiral the engine draws.

use nice_plug::{editor::dpi::LogicalSize, prelude::*};
use nice_plug_iced::{
	IcedEditor, IcedEditorState, IcedNiceContext, IcedNiceSettings, PersistentState, application,
	create_iced_editor,
	iced::{
		self, ContentFit, Element, Length, PollSubNotifier, Subscription, Theme,
		widget::{center, image, text},
	},
};
use parking_lot::Mutex;
use std::sync::Arc;

use crate::engine::Engine;
use crate::params::PluginParams;

const DEFAULT_SIZE: LogicalSize<f32> = LogicalSize::new(1000.0, 700.0);
const MIN_SIZE: LogicalSize<f32> = LogicalSize::new(400.0, 300.0);

pub fn default_state() -> Arc<IcedEditorState> {
	IcedEditorState::from_size(DEFAULT_SIZE, 1.0)
}

/// What the editor reads and drives, shared with the plugin.
pub struct Shared {
	params: Arc<PluginParams>,
	engine: Arc<Mutex<Option<Engine>>>,
}

pub fn create(
	state: Arc<IcedEditorState>,
	params: Arc<PluginParams>,
	engine: Arc<Mutex<Option<Engine>>>,
	notifier: PollSubNotifier,
) -> Option<IcedEditor> {
	create_iced_editor(
		state,
		Shared { params, engine },
		notifier,
		IcedNiceSettings::new()
			.with_resize_hint(ResizeHint::resizable().with_min_logical_size(MIN_SIZE)),
		|shared, ctx| {
			Ok(application(shared, ctx, Gui::new, Gui::update, Gui::view)
				.theme(|_: &Gui| Some(Theme::Dark))
				.subscription(Gui::subscription)
				.run())
		},
	)
}

#[derive(Debug, Clone, Copy)]
enum Message {
	/// Woken to draw a frame.
	Frame,
	WindowResized,
}

struct Gui {
	shared: PersistentState<Shared>,
	ctx: IcedNiceContext,
	/// The last frame the engine drew, or none while it is not running.
	frame: Option<image::Handle>,
}

impl Gui {
	fn new(shared: PersistentState<Shared>, ctx: IcedNiceContext) -> Self {
		Self {
			shared,
			ctx,
			frame: None,
		}
	}

	fn subscription(&self) -> Subscription<Message> {
		Subscription::batch([
			iced::poll_events().map(|_| Message::Frame),
			iced::window_resized().map(|_| Message::WindowResized),
		])
	}

	fn update(&mut self, message: Message) {
		match message {
			Message::Frame => self.draw(),
			Message::WindowResized => {
				// Keeps the editor's size current so it reopens at the size the
				// host last gave it.
				self.ctx.sync_window_size();
				self.draw();
			}
		}
	}

	/// Applies the parameters to the engine and has it draw a frame the size of
	/// the window.
	fn draw(&mut self) {
		let config = self.shared.params.config();
		let size = self.ctx.window_size().physical;
		let mut engine = self.shared.engine.lock();
		let Some(engine) = engine.as_mut() else {
			self.frame = None;
			return;
		};
		match engine
			.apply(&config)
			.and_then(|()| engine.render(size.width, size.height))
		{
			Ok(graphic) => {
				let (width, height, pixels) = graphic.into_rgba();
				self.frame = Some(image::Handle::from_rgba(width, height, pixels));
			}
			Err(err) => {
				// Logged once, not once a frame, until a frame succeeds again.
				if self.frame.take().is_some() {
					log::error!("failed to draw a frame: {err}");
				}
			}
		}
	}

	fn view(&self) -> Element<'_, Message> {
		let Some(frame) = &self.frame else {
			return center(text("Melody Visualizer").size(24))
				.width(Length::Fill)
				.height(Length::Fill)
				.into();
		};
		// The frame is drawn at the window's size in physical pixels, so it fills
		// the window one to one.
		image(frame.clone())
			.width(Length::Fill)
			.height(Length::Fill)
			.content_fit(ContentFit::Fill)
			.filter_method(image::FilterMethod::Nearest)
			.into()
	}
}
