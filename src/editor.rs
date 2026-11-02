//! The plugin's window: the spiral the engine draws, with the interval ring's
//! labels over it, and the control pane beside it.

use nice_plug::{editor::dpi::LogicalSize, prelude::*};
use nice_plug_iced::{
	IcedEditor, IcedEditorState, IcedNiceContext, IcedNiceSettings, PersistentState, application,
	create_iced_editor,
	iced::{
		self, Color, ContentFit, Element, Font, Length, Point, PollSubNotifier, Rectangle,
		Renderer, Subscription, Task, Theme, alignment, font, mouse,
		widget::{Canvas, canvas, center, image, row, stack, text},
	},
};
use parking_lot::Mutex;
use std::f32::consts::TAU;
use std::sync::Arc;

use crate::app::config::GraphicGeneratorConfig;
use crate::engine::Engine;
use crate::graphic::generators::spiral;
use crate::params::PluginParams;
use pane::Stage;

mod pane;
mod range_slider;

const DEFAULT_SIZE: LogicalSize<f32> = LogicalSize::new(1200.0, 760.0);
/// Room for the pane and a spiral beside it.
const MIN_SIZE: LogicalSize<f32> = LogicalSize::new(pane::WIDTH + 400.0, 400.0);

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
				.theme(|_: &Gui| Some(pane::theme()))
				.subscription(Gui::subscription)
				.run())
		},
	)
}

#[derive(Debug, Clone)]
pub enum Message {
	/// Woken to draw a frame.
	Frame,
	/// The frame last drawn is on the GPU, ready to show.
	Uploaded(Result<image::Allocation, image::Error>),
	WindowResized,
	/// A stage's row was clicked: open the stage, or close it if it is open.
	Open(Stage),
	/// A slider moved the parameter to a normalized value.
	Drag(ParamPtr, f32),
	/// The slider being dragged was let go.
	Release,
	/// A switch or a button set the parameter to a normalized value at once.
	Click(ParamPtr, f32),
}

struct Gui {
	shared: PersistentState<Shared>,
	ctx: IcedNiceContext,
	/// The frame on show, or none while the engine is not running. Holding its
	/// allocation is what keeps it on the GPU.
	frame: Option<image::Allocation>,
	/// Whether a frame the engine drew is still on its way to the GPU.
	uploading: bool,
	/// The labels to draw over the frame, when the ring is on.
	ring: Option<IntervalRing>,
	/// The stage whose controls show in the pane.
	open: Option<Stage>,
	/// Whether the engine draws MIDI notes rather than audio, as of the last
	/// frame.
	midi: bool,
	/// The parameter a slider is dragging, between the host being told the
	/// gesture began and that it ended.
	dragging: Option<ParamPtr>,
}

impl Gui {
	fn new(shared: PersistentState<Shared>, ctx: IcedNiceContext) -> Self {
		Self {
			shared,
			ctx,
			frame: None,
			uploading: false,
			ring: None,
			open: None,
			midi: false,
			dragging: None,
		}
	}

	fn subscription(&self) -> Subscription<Message> {
		Subscription::batch([
			iced::poll_events().map(|_| Message::Frame),
			iced::window_resized().map(|_| Message::WindowResized),
		])
	}

	fn update(&mut self, message: Message) -> Task<Message> {
		match message {
			Message::Frame => return self.draw(),
			Message::Uploaded(result) => {
				self.uploading = false;
				match result {
					Ok(frame) => self.frame = Some(frame),
					Err(err) => log::error!("failed to upload a frame: {err}"),
				}
			}
			Message::WindowResized => {
				// Keeps the editor's size current so it reopens at the size the
				// host last gave it.
				self.ctx.sync_window_size();
				return self.draw();
			}
			Message::Open(stage) => {
				self.open = (self.open != Some(stage)).then_some(stage);
			}
			Message::Drag(param, normalized) => {
				if self.dragging != Some(param) {
					self.end_drag();
					self.dragging = Some(param);
					// SAFETY: every `ParamPtr` comes from `self.shared.params`, which
					// the editor keeps alive.
					unsafe { self.ctx.nice_context.raw_begin_set_parameter(param) };
				}
				// SAFETY: as above.
				unsafe {
					self.ctx
						.nice_context
						.raw_set_parameter_normalized(param, normalized)
				};
			}
			Message::Release => self.end_drag(),
			Message::Click(param, normalized) => {
				// SAFETY: as for `Drag`.
				unsafe {
					let context = &self.ctx.nice_context;
					context.raw_begin_set_parameter(param);
					context.raw_set_parameter_normalized(param, normalized);
					context.raw_end_set_parameter(param);
				}
			}
		}
		Task::none()
	}

	/// Tells the host the drag under way, if any, has ended.
	fn end_drag(&mut self) {
		if let Some(param) = self.dragging.take() {
			// SAFETY: as for `Drag`.
			unsafe { self.ctx.nice_context.raw_end_set_parameter(param) };
		}
	}

	/// Applies the parameters to the engine, has it draw a frame the size of the
	/// window's room beside the pane, in physical pixels, and sends the frame to
	/// the GPU.
	///
	/// The frame on show stays until the new one has landed: iced uploads a frame
	/// this large off the GPU's thread, and draws nothing for it until then. No
	/// new frame is drawn while one is uploading, so frames never queue up.
	fn draw(&mut self) -> Task<Message> {
		if self.uploading {
			return Task::none();
		}
		let mut config = self.shared.params.config();
		let window = self.ctx.window_size();
		let scale = window.physical.width as f64 / window.logical.width;
		let mut size = window.physical;
		size.width = size
			.width
			.saturating_sub((f64::from(pane::WIDTH) * scale).round() as u32);
		let mut engine = self.shared.engine.lock();
		let Some(engine) = engine.as_mut() else {
			self.frame = None;
			return Task::none();
		};
		self.midi = engine.hears_midi();
		if self.midi {
			config.for_midi();
		}

		let GraphicGeneratorConfig::Spiral(spiral) = &config.graphic_generator;
		self.ring = spiral.interval_ring.then(|| IntervalRing {
			radius: (spiral::outer_radius(spiral, size.width as i32, size.height as i32)
				+ LABEL_OFFSET) as f32,
			width: size.width as f32,
		});

		match engine
			.apply(&config)
			.and_then(|()| engine.render(size.width, size.height))
		{
			Ok(graphic) => {
				let (width, height, pixels) = graphic.into_rgba();
				self.uploading = true;
				image::allocate(image::Handle::from_rgba(width, height, pixels))
					.map(Message::Uploaded)
			}
			Err(err) => {
				// Logged once, not once a frame, until a frame succeeds again.
				if self.frame.take().is_some() {
					log::error!("failed to draw a frame: {err}");
				}
				Task::none()
			}
		}
	}

	fn view(&self) -> Element<'_, Message> {
		row![
			self.visualization(),
			pane::view(&self.shared.params, self.open, self.midi)
		]
		.into()
	}

	fn visualization(&self) -> Element<'_, Message> {
		let Some(frame) = &self.frame else {
			return center(text("Melody Visualizer").size(24))
				.width(Length::Fill)
				.height(Length::Fill)
				.into();
		};
		// The frame is drawn at its area's size in physical pixels, so it fills
		// the area one to one.
		let frame = image(frame.handle().clone())
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
