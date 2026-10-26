//! The plugin's window. For now it is an empty iced editor.

use nice_plug::{editor::dpi::LogicalSize, prelude::*};
use nice_plug_iced::{
	IcedEditor, IcedEditorState, IcedNiceContext, IcedNiceSettings, PersistentState, application,
	create_iced_editor,
	iced::{
		self, Element, Length, PollSubNotifier, Subscription, Theme,
		widget::{center, text},
	},
};
use std::sync::Arc;

const DEFAULT_SIZE: LogicalSize<f32> = LogicalSize::new(1000.0, 700.0);
const MIN_SIZE: LogicalSize<f32> = LogicalSize::new(400.0, 300.0);

pub fn default_state() -> Arc<IcedEditorState> {
	IcedEditorState::from_size(DEFAULT_SIZE, 1.0)
}

pub fn create(state: Arc<IcedEditorState>) -> Option<IcedEditor> {
	create_iced_editor(
		state,
		(),
		PollSubNotifier::new(),
		IcedNiceSettings::new()
			.with_resize_hint(ResizeHint::resizable().with_min_logical_size(MIN_SIZE)),
		|persistent_state, ctx| {
			Ok(
				application(persistent_state, ctx, Gui::new, Gui::update, Gui::view)
					.theme(|_: &Gui| Some(Theme::Dark))
					.subscription(Gui::subscription)
					.run(),
			)
		},
	)
}

#[derive(Debug, Clone, Copy)]
enum Message {
	WindowResized,
}

struct Gui {
	ctx: IcedNiceContext,
}

impl Gui {
	fn new(_state: PersistentState<()>, ctx: IcedNiceContext) -> Self {
		Self { ctx }
	}

	fn subscription(&self) -> Subscription<Message> {
		iced::window_resized().map(|_| Message::WindowResized)
	}

	fn update(&mut self, message: Message) {
		match message {
			// Keeps the editor's size current so it reopens at the size the
			// host last gave it.
			Message::WindowResized => self.ctx.sync_window_size(),
		}
	}

	fn view(&self) -> Element<'_, Message> {
		center(text("Melody Visualizer").size(24))
			.width(Length::Fill)
			.height(Length::Fill)
			.into()
	}
}
