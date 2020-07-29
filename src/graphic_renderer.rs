use futures::{prelude::*, channel::mpsc, executor, select};
use log::{debug, error};
use std::any::Any;
use std::collections::VecDeque;
use std::fmt::Debug;
use std::thread;
use std::time::Duration;
use std::sync::Arc;

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
	fn generate(
		&mut self,
		buffer: GraphicBuffer,
		params: &Arc<SpectrumParams>,
		spectrum_history: &VecDeque<Spectrum>,
	) -> Result<Graphic, Error>;

	fn history_len(&self) -> usize;

	fn upcast_any_ref(&self) -> &dyn Any;
	fn upcast_any_mut(&mut self) -> &mut dyn Any;
}

#[derive(Debug)]
pub struct DefaultGraphicGenerator;

impl GraphicGenerator for DefaultGraphicGenerator {
	fn generate(
		&mut self,
		buffer: GraphicBuffer,
		_params: &Arc<SpectrumParams>,
		_spectrum_history: &VecDeque<Spectrum>
	) -> Result<Graphic, Error>
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

	fn upcast_any_ref(&self) -> &dyn Any {
		self
	}

	fn upcast_any_mut(&mut self) -> &mut dyn Any {
		self
	}
}

pub struct GraphicRenderer {
	generator: Box<dyn GraphicGenerator>,
	spectrum_history: VecDeque<Spectrum>,
	interval: Duration,
	spectrum_params: Arc<SpectrumParams>,
}

impl GraphicRenderer {
	fn new() -> Self {
		GraphicRenderer {
			generator: Box::new(DefaultGraphicGenerator),
			spectrum_history: VecDeque::new(),
			interval: Duration::from_millis(40),
			spectrum_params: Arc::new(SpectrumParams::default()),
		}
	}

	fn update_spectrum(&mut self, spectrum: Spectrum) -> SpectrumBuffer {
		let max_history_len = self.generator.history_len();
		self.spectrum_history.truncate(max_history_len);
		if Arc::ptr_eq(spectrum.params(), &self.spectrum_params) {
			self.spectrum_history.push_front(spectrum);
		}
		if self.spectrum_history.len() > max_history_len {
			self.spectrum_history.pop_back()
				.expect("spectrum_history len is greater than 0")
				.into_buffer()
		} else {
			self.new_spectrum_buffer()
		}
	}

	fn new_spectrum_buffer(&self) -> SpectrumBuffer {
		SpectrumBuffer::new(self.spectrum_params.clone())
	}

	fn frame_interval(&self) -> Duration {
		self.interval
	}

	fn set_frame_interval(&mut self, interval: Duration) {
		self.interval = interval;
	}

	pub fn render(&mut self, buffer: GraphicBuffer) -> Result<Graphic, Error> {
		self.generator.generate(buffer, &self.spectrum_params, &self.spectrum_history)
	}

	pub fn generator(&self) -> &dyn GraphicGenerator {
		&*self.generator
	}

	pub fn generator_mut(&mut self) -> &mut dyn GraphicGenerator {
		&mut *self.generator
	}

	pub fn set_generator(&mut self, generator: Box<dyn GraphicGenerator>) {
		self.generator = generator;
	}

	pub fn spectrum_params(&self) -> &Arc<SpectrumParams> {
		&self.spectrum_params
	}

	pub fn set_spectrum_params(&mut self, params: SpectrumParams) {
		self.spectrum_params = Arc::new(params);
		self.spectrum_history.clear();
	}
}

pub fn start(
	spectrum_input: mpsc::Receiver<Spectrum>,
	spectrum_output: mpsc::Sender<SpectrumBuffer>,
) -> Result<AsyncProcessor<GraphicRenderer>, Error>
{
	start_with_thread_name(
		"GraphicProcessor".into(),
		spectrum_input,
		spectrum_output,
	)
}

pub fn start_with_thread_name(
	name: String,
	spectrum_input: mpsc::Receiver<Spectrum>,
	spectrum_output: mpsc::Sender<SpectrumBuffer>,
) -> Result<AsyncProcessor<GraphicRenderer>, Error>
{
	let (exec_tx, exec_rx) = mpsc::channel(0);
	let mut processor = GraphicProcessor::new(
		exec_rx,
		spectrum_input,
		spectrum_output,
	);
	let _ = thread::Builder::new()
		.name(name)
		.spawn(move || executor::block_on(processor.process_loop()))?;
	Ok(AsyncProcessor::new(exec_tx))
}

struct GraphicProcessor {
	exec_rx: mpsc::Receiver<Box<dyn FnOnce(&mut GraphicRenderer) + Send>>,
	spectrum_input: mpsc::Receiver<Spectrum>,
	spectrum_output: mpsc::Sender<SpectrumBuffer>,
	current_buffer: Option<GraphicBuffer>,
	renderer: GraphicRenderer,
}

impl GraphicProcessor {
	fn new(
		exec_rx: mpsc::Receiver<Box<dyn FnOnce(&mut GraphicRenderer) + Send>>,
		spectrum_input: mpsc::Receiver<Spectrum>,
		spectrum_output: mpsc::Sender<SpectrumBuffer>,
	) -> Self {
		GraphicProcessor {
			exec_rx,
			spectrum_input,
			spectrum_output,
			current_buffer: None,
			renderer: GraphicRenderer::new(),
		}
	}

	async fn process_loop(&mut self) {
		debug!("Starting graphic rendering thread");
		self.current_buffer = Some(GraphicBuffer::default());

		// Kick off the spectrum generation loop.
		match self.send_spectrum_buffer(self.renderer.new_spectrum_buffer()).await {
			Ok(true) => {},
			_ => {
				error!(
					"failed to send initial spectrum buffer to processing thread, \
					exiting graphic rendering thread"
				);
				return;
			}
		}

		loop {
			let result = select! {
				exec = self.exec_rx.next() => self.handle_exec(exec),
				new_spectrum = self.spectrum_input.next() =>
					self.handle_new_spectrum(new_spectrum).await,
			};
			match result {
				Ok(true) => {},
				Ok(false) => break,
				Err(err) => error!("error during graphic render processing: {}", err),
			}
		}
		debug!("Exiting graphic rendering thread");
	}

	fn handle_exec(&mut self, exec: Option<Box<dyn FnOnce(&mut GraphicRenderer) + Send>>)
		-> Result<bool, GraphicProcessingError>
	{
		if let Some(exec) = exec {
			exec(&mut self.renderer);
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
			self.send_spectrum_buffer(buffer).await
		} else {
			debug!("spectrum input channel closed, stopping graphic processing");
			Ok(false)
		}
	}

	async fn send_spectrum_buffer(&mut self, buffer: SpectrumBuffer)
		-> Result<bool, GraphicProcessingError>
	{
		if let Err(err) = self.spectrum_output.send(buffer).await {
			return if err.is_disconnected() {
				debug!("spectrum output channel disconnected, stopping graphic processing");
				Ok(false)
			} else {
				Err(err.into())
			};
		}
		Ok(true)
	}
}
