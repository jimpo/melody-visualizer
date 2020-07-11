use futures::{prelude::*, channel::{mpsc, oneshot}, executor, select};
use futures_timer::Delay;
use log::{debug, error};
use std::any::Any;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::thread;
use std::time::{Duration, Instant};

use crate::async_processor::AsyncProcessor;
use crate::graphic::{Graphic, GraphicBuffer};
use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumParams};
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

#[derive(Debug)]
pub enum GraphicRendererCmd {
	SetSpectrumParams(SpectrumParams),
}

pub fn start(
	graphic_input: mpsc::Receiver<GraphicBuffer>,
	graphic_output: mpsc::Sender<Graphic>,
	spectrum_input: mpsc::Receiver<Spectrum>,
	spectrum_output: mpsc::Sender<SpectrumBuffer>,
) -> Result<AsyncProcessor<GraphicRendererCmd>, Error>
{
	start_with_thread_name(
		"GraphicProcessor".into(),
		graphic_input,
		graphic_output,
		spectrum_input,
		spectrum_output,
	)
}

pub fn start_with_thread_name(
	name: String,
	graphic_input: mpsc::Receiver<GraphicBuffer>,
	graphic_output: mpsc::Sender<Graphic>,
	spectrum_input: mpsc::Receiver<Spectrum>,
	spectrum_output: mpsc::Sender<SpectrumBuffer>,
) -> Result<AsyncProcessor<GraphicRendererCmd>, Error>
{
	let (control_tx, control_rx) = mpsc::channel(0);
	let mut processor = GraphicProcessor::new(
		control_rx,
		graphic_input,
		graphic_output,
		spectrum_input,
		spectrum_output,
	);
	let _ = thread::Builder::new()
		.name(name)
		.spawn(move || executor::block_on(processor.process_loop()))?;
	Ok(AsyncProcessor::new(control_tx))
}

struct GraphicProcessor {
	control_rx: mpsc::Receiver<(GraphicRendererCmd, oneshot::Sender<Box<dyn Any + Send>>)>,
	graphic_input: mpsc::Receiver<GraphicBuffer>,
	graphic_output: mpsc::Sender<Graphic>,
	spectrum_input: mpsc::Receiver<Spectrum>,
	spectrum_output: mpsc::Sender<SpectrumBuffer>,
	current_buffer: Option<GraphicBuffer>,
	next_tick_time: Instant,
	renderer: GraphicRenderer,
}

impl GraphicProcessor {
	fn new(
		control_rx: mpsc::Receiver<(GraphicRendererCmd, oneshot::Sender<Box<dyn Any + Send>>)>,
		graphic_input: mpsc::Receiver<GraphicBuffer>,
		graphic_output: mpsc::Sender<Graphic>,
		spectrum_input: mpsc::Receiver<Spectrum>,
		spectrum_output: mpsc::Sender<SpectrumBuffer>,
	) -> Self {
		GraphicProcessor {
			control_rx,
			graphic_input,
			graphic_output,
			spectrum_input,
			spectrum_output,
			current_buffer: None,
			next_tick_time: Instant::now(),
			renderer: GraphicRenderer::new(),
		}
	}

	async fn process_loop(&mut self) {
		debug!("Starting graphic rendering thread");
		self.current_buffer = Some(GraphicBuffer::default());
		self.next_tick_time = Instant::now();
		loop {
			let tick_delay = self.next_tick_time.saturating_duration_since(Instant::now());
			let result = select! {
				cmd = self.control_rx.next() => self.handle_cmd(cmd).await,
				new_buffer = self.graphic_input.next() => self.handle_new_buffer(new_buffer).await,
				new_spectrum = self.spectrum_input.next() =>
					self.handle_new_spectrum(new_spectrum).await,
				_ = Delay::new(tick_delay).fuse() => self.handle_tick().await,
			};
			match result {
				Ok(true) => {},
				Ok(false) => break,
				Err(err) => error!("error during graphic render processing: {}", err),
			}
		}
		debug!("Exiting graphic rendering thread");
	}

	async fn handle_cmd(
		&mut self,
		request: Option<(GraphicRendererCmd, oneshot::Sender<Box<dyn Any + Send>>)>,
	) -> Result<bool, GraphicProcessingError>
	{
		if let Some((cmd, reply_tx)) = request {
			debug!("graphic rendering thread received command: {:?}", cmd);
			let result = Box::new(());
			if let Err(err) = reply_tx.send(result) {
				debug!("RPC response channel disconnected");
			}
			Ok(true)
		} else {
			debug!("graphic control channel disconnected, stopping graphic processing");
			Ok(false)
		}
	}

	async fn handle_new_spectrum(&mut self, spectrum: Option<Spectrum>)
		-> Result<bool, GraphicProcessingError>
	{
		if let Some(spectrum) = spectrum {
			let buffer = self.renderer.update_spectrum(spectrum);
			if let Err(err) = self.spectrum_output.send(buffer).await {
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

	async fn handle_new_buffer(&mut self, new_buffer: Option<GraphicBuffer>)
		-> Result<bool, GraphicProcessingError>
	{
		if let Some(new_buffer) = new_buffer {
			if self.current_buffer.is_some() {
				Err(GraphicProcessingError::ReceivedUnexpectedBuffer)
			} else {
				self.current_buffer = Some(new_buffer);
				Ok(true)
			}
		} else {
			debug!("graphic input channel closed, stopping graphic processing");
			Ok(false)
		}
	}

	async fn handle_tick(&mut self) -> Result<bool, GraphicProcessingError> {
		self.next_tick_time += self.renderer.frame_interval();

		if let Some(buffer) = self.current_buffer.take() {
			let graphic = self.renderer.render(buffer)?;
			if let Err(err) = self.graphic_output.send(graphic).await {
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
}
