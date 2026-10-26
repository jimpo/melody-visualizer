//! The plugin's window: the spiral the engine draws, with the interval ring's
//! labels over it.

use nice_plug::{editor::dpi::LogicalSize, prelude::*};
use nice_plug_iced::{
	IcedEditor, IcedEditorState, IcedNiceContext, IcedNiceSettings, PersistentState, application,
	create_iced_editor,
	iced::{
		self, Color, ContentFit, Element, Font, Length, Point, PollSubNotifier, Rectangle,
		Renderer, Subscription, Theme, alignment, font, mouse,
		widget::{Canvas, canvas, center, image, stack, text},
	},
};
use parking_lot::Mutex;
use std::f32::consts::TAU;
use std::sync::Arc;

use crate::app::config::GraphicGeneratorConfig;
use crate::engine::Engine;
use crate::graphic::generators::spiral;
use crate::params::PluginParams;

const DEFAULT_SIZE: LogicalSize<f32> = LogicalSize::new(1000.0, 700.0);
const MIN_SIZE: LogicalSize<f32> = LogicalSize::new(400.0, 300.0);

/// The interval names of the twelve spokes, clockwise from the key.
const INTERVALS: [&str; 12] = [
	"P1", "m2", "M2", "m3", "M3", "P4", "TT", "P5", "m6", "M6", "m7", "M7",
];
/// Where a label's centre sits, in pixels outside the outermost turn.
const LABEL_OFFSET: f64 = 20.0;
/// Matched among the installed fonts; without it the labels fall back to the
/// default face.
const LABEL_FONT: Font = Font {
	weight: font::Weight::Medium,
	..Font::with_name("Playfair Display")
};
const LABEL_SIZE: f32 = 13.0;
const LABEL_ALPHA: f32 = 0.55;

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
	/// The labels to draw over the frame, when the ring is on.
	ring: Option<IntervalRing>,
}

impl Gui {
	fn new(shared: PersistentState<Shared>, ctx: IcedNiceContext) -> Self {
		Self {
			shared,
			ctx,
			frame: None,
			ring: None,
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

		let GraphicGeneratorConfig::Spiral(spiral) = &config.graphic_generator;
		self.ring = spiral.interval_ring.then(|| IntervalRing {
			radius: (spiral::outer_radius(spiral, size.width as i32, size.height as i32)
				+ LABEL_OFFSET) as f32,
			width: size.width as f32,
		});
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
		let frame = image(frame.clone())
			.width(Length::Fill)
			.height(Length::Fill)
			.content_fit(ContentFit::Fill)
			.filter_method(image::FilterMethod::Nearest);
		match &self.ring {
			Some(ring) => stack![
				frame,
				Canvas::new(ring).width(Length::Fill).height(Length::Fill)
			]
			.into(),
			None => frame.into(),
		}
	}
}

/// The interval names around the outermost turn, upright, one per spoke.
struct IntervalRing {
	/// The labels' distance from the centre, in the frame's physical pixels.
	radius: f32,
	/// The frame's width in physical pixels, which scales the radius to the
	/// canvas's logical ones.
	width: f32,
}

impl canvas::Program<Message> for IntervalRing {
	type State = ();

	fn draw(
		&self,
		_state: &(),
		renderer: &Renderer,
		_theme: &Theme,
		bounds: Rectangle,
		_cursor: mouse::Cursor,
	) -> Vec<canvas::Geometry> {
		let mut frame = canvas::Frame::new(renderer, bounds.size());
		let radius = self.radius * bounds.width / self.width;
		let centre = frame.center();
		for (step, name) in INTERVALS.iter().enumerate() {
			let (sin, cos) = (step as f32 / 12.0 * TAU).sin_cos();
			frame.fill_text(canvas::Text {
				content: name.to_string(),
				position: Point::new(centre.x + sin * radius, centre.y - cos * radius),
				color: Color {
					a: LABEL_ALPHA,
					..Color::WHITE
				},
				size: LABEL_SIZE.into(),
				font: LABEL_FONT,
				align_x: alignment::Horizontal::Center.into(),
				align_y: alignment::Vertical::Center,
				..canvas::Text::default()
			});
		}
		vec![frame.into_geometry()]
	}
}
