use futures::{prelude::*, channel::mpsc, executor, select};
use futures_timer::Delay;
use log::{debug, error};
use std::collections::VecDeque;
use std::fmt::Debug;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::graphic::{Graphic, GraphicBuffer};
use crate::spectrum::{Spectrum, SpectrumBuffer};
use crate::error::Error;

// Graphics renderer
//
// Periodically renders new surface, pushes to output channel,
//   reads in new surface from input channel.
// Select over timer, input spectrum buffer + next buffer time estimate, surface.
//

// GraphicsRenderer -> SpectrumBuffer -> SpectrumRenderer -> Spectrum -> GraphicsRenderer |
// ^                                                                                      |
// |                                                                                      |
// ________________________________________________________________________________________

// ProcessingComponents
//
// Q's: Where are the spectrum params decided? It must be the SpectrumRenderer. Or set on both
// processing components.
//
// Control must be an RPC interface to handle dynamic commands. Commands are Box::Any.
//
// RPC: async fn call<R>(&mut self, cmd: C) -> Result<R, Error>
// async fn stop(&mut self) -> Result<(), Error>
//
// glib::Sender<Box<dyn Any>>
//
//
//

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From)]
enum GraphicProcessingError {
	SendError(mpsc::SendError),
	#[display(fmt = "skipping tick because no buffer is available")]
	NoBuffer,
	#[display(fmt = "received an unexpected buffer while one is already available")]
	ReceivedUnexpectedBuffer,
	Other(Error),
}

pub trait GraphicGenerator: Debug + Send {
	fn generate(&mut self, buffer: GraphicBuffer, spectrum_history: &VecDeque<Spectrum>)
		-> Result<Graphic, Error>;
	fn history_len(&self) -> usize;
}

#[derive(Debug)]
pub struct DefaultGraphicGenerator;

impl GraphicGenerator for DefaultGraphicGenerator {
	fn generate(&mut self, buffer: GraphicBuffer, _spectrum_history: &VecDeque<Spectrum>)
		-> Result<Graphic, Error>
	{
		let x_max = buffer.width();
		let y_max = buffer.height();

		buffer.draw(|ctx| {
			ctx.set_source_rgb(0.0, 0.0, 0.0);
			ctx.rectangle(0.0, 0.0, x_max as f64, y_max as f64);
			ctx.fill();
			Ok(())
		})
	}

	fn history_len(&self) -> usize {
		1
	}
}

struct GraphicRenderer {
	generator: Box<dyn GraphicGenerator>,
	spectrum_history: VecDeque<Spectrum>,
	interval: Duration,
}

impl GraphicRenderer {
	fn new() -> Self {
		GraphicRenderer {
			generator: Box::new(DefaultGraphicGenerator),
			spectrum_history: VecDeque::new(),
			interval: Duration::from_millis(40),
		}
	}

	fn render(&mut self, buffer: GraphicBuffer) -> Result<Graphic, Error> {
		self.generator.generate(buffer, &self.spectrum_history)
	}

	fn update_spectrum(&mut self, spectrum: Spectrum) -> SpectrumBuffer {
		self.spectrum_history.truncate(self.generator.history_len());
		let buffer = self.spectrum_history.pop_back()
			.map(Spectrum::into_buffer)
			.unwrap_or_else(SpectrumBuffer::default);
		self.spectrum_history.push_front(spectrum);
		buffer
	}

	fn frame_interval(&self) -> Duration {
		self.interval
	}

	fn set_frame_interval(&mut self, interval: Duration) {
		self.interval = interval;
	}
}

pub struct AsyncGraphicRenderer {
	thread: Option<JoinHandle<()>>,
	control_tx: mpsc::Sender<GraphicRendererCmd>,
}

#[derive(Debug)]
enum GraphicRendererCmd {
}

impl AsyncGraphicRenderer {
	pub fn new(
		graphic_input: mpsc::Receiver<GraphicBuffer>,
		graphic_output: mpsc::Sender<Graphic>,
		spectrum_input: mpsc::Receiver<Spectrum>,
		spectrum_output: mpsc::Sender<SpectrumBuffer>,
	) -> Result<Self, Error>	{
		Self::with_thread_name(
			"AsyncGraphicRenderer".into(),
			graphic_input,
			graphic_output,
			spectrum_input,
			spectrum_output,
		)
	}

	pub fn with_thread_name(
		name: String,
		graphic_input: mpsc::Receiver<GraphicBuffer>,
		graphic_output: mpsc::Sender<Graphic>,
		spectrum_input: mpsc::Receiver<Spectrum>,
		spectrum_output: mpsc::Sender<SpectrumBuffer>,
	) -> Result<Self, Error> {
		let (control_tx, control_rx) = mpsc::channel(0);
		let processing_thread = thread::Builder::new()
			.name(name)
			.spawn(move || {
				executor::block_on(process_loop(
					control_rx,
					graphic_input,
					graphic_output,
					spectrum_input,
					spectrum_output,
				));
			})?;
		Ok(AsyncGraphicRenderer {
			thread: Some(processing_thread),
			control_tx,
		})
	}

	// async fn update params
	pub async fn stop(&mut self) -> Result<(), Error> {
		if let Err(err) = self.control_tx.close().await {
			if !err.is_disconnected() {
				return Err(Error::ProcessingControlError(err));
			}
		}
		Ok(())
	}
}

async fn process_loop(
	mut control_rx: mpsc::Receiver<GraphicRendererCmd>,
	mut graphic_input: mpsc::Receiver<GraphicBuffer>,
	mut graphic_output: mpsc::Sender<Graphic>,
	mut spectrum_input: mpsc::Receiver<Spectrum>,
	mut spectrum_output: mpsc::Sender<SpectrumBuffer>,
) {
	debug!("Starting graphic rendering thread");
	let mut renderer = GraphicRenderer::new();
	let mut buffer = Some(GraphicBuffer::default());
	let mut next_tick_time = Instant::now();
	loop {
		let tick_delay = next_tick_time.saturating_duration_since(Instant::now());
		let result = select! {
			cmd = control_rx.next() => handle_cmd(&mut renderer, cmd).await,
			new_buffer = graphic_input.next() =>
				handle_new_buffer(&mut renderer, &mut buffer, new_buffer).await,
			new_spectrum = spectrum_input.next() =>
				handle_new_spectrum(&mut renderer, new_spectrum, &mut spectrum_output).await,
			_ = Delay::new(tick_delay).fuse() => handle_tick(
				&mut renderer,
				&mut next_tick_time,
				buffer.take(),
				&mut graphic_output,
			).await,
		};
		match result {
			Ok(true) => {},
			Ok(false) => break,
			Err(err) => error!("error during graphic render processing: {}", err),
		}
	}
	debug!("Exiting graphic rendering thread");
}

async fn handle_cmd(renderer: &mut GraphicRenderer, cmd: Option<GraphicRendererCmd>)
	-> Result<bool, GraphicProcessingError>
{
	debug!("graphic rendering thread received command: {:?}", cmd);
	match cmd {
		Some(_) => Ok(true),
		None => Ok(false),
	}
}

async fn handle_new_spectrum(
	renderer: &mut GraphicRenderer,
	spectrum: Option<Spectrum>,
	spectrum_output: &mut mpsc::Sender<SpectrumBuffer>,
) -> Result<bool, GraphicProcessingError>
{
	if let Some(spectrum) = spectrum {
		let buffer = renderer.update_spectrum(spectrum);
		if let Err(err) = spectrum_output.send(buffer).await {
			return if err.is_disconnected() {
				debug!("spectrum output channel disconnected, stopping graphic processing");
				Ok(false)
			} else {
				Err(err.into())
			};
		}
		Ok(true)
	} else {
		debug!("spectrum input channel closed, stopping graphic processing");
		Ok(false)
	}
}

async fn handle_new_buffer(
	renderer: &mut GraphicRenderer,
	current_buffer: &mut Option<GraphicBuffer>,
	new_buffer: Option<GraphicBuffer>,
) -> Result<bool, GraphicProcessingError>
{
	if let Some(new_buffer) = new_buffer {
		if current_buffer.is_some() {
			Err(GraphicProcessingError::ReceivedUnexpectedBuffer)
		} else {
			*current_buffer = Some(new_buffer);
			Ok(true)
		}
	} else {
		debug!("graphic input channel closed, stopping graphic processing");
		Ok(false)
	}
}

async fn handle_tick(
	renderer: &mut GraphicRenderer,
	next_tick_time: &mut Instant,
	buffer: Option<GraphicBuffer>,
	graphic_output: &mut mpsc::Sender<Graphic>,
) -> Result<bool, GraphicProcessingError>
{
	*next_tick_time += renderer.frame_interval();

	if let Some(buffer) = buffer {
		let graphic = renderer.render(buffer)?;
		if let Err(err) = graphic_output.send(graphic).await {
			return if err.is_disconnected() {
				debug!("graphic output channel disconnected, stopping graphic processing");
				Ok(false)
			} else {
				Err(err.into())
			};
		}
		Ok(true)
	} else {
		Err(GraphicProcessingError::NoBuffer)
	}
}
