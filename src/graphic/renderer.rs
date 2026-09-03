use futures::{channel::mpsc, executor, prelude::*, select};
use log::{debug, error};
use std::{any::Any, collections::VecDeque, sync::Arc, thread};

use crate::async_processor::{AsyncProcessor, ExecCommand, ExecReceiver};
use crate::error::Error;
use crate::graphic::{Graphic, GraphicBuffer, GraphicGenerator};
use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumParams};

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
	Other(Error),
}

#[derive(Debug)]
pub struct DefaultGraphicGenerator;

impl GraphicGenerator for DefaultGraphicGenerator {
	fn generate(
		&mut self,
		buffer: GraphicBuffer,
		_spectrum_history: &VecDeque<Spectrum>,
	) -> Result<Graphic, Error> {
		let x_max = buffer.width();
		let y_max = buffer.height();

		buffer.draw(|ctx| {
			ctx.set_source_rgb(0.0, 0.0, 0.0);
			ctx.rectangle(0.0, 0.0, x_max as f64, y_max as f64);
			ctx.fill()?;
			Ok(())
		})
	}

	fn history_len(&self) -> usize {
		1
	}

	fn as_any_mut(&mut self) -> &mut dyn Any {
		self
	}
}

/// Drives a [`GraphicGenerator`]: keeps the recent spectra, owns the frequency
/// grid and the surface size, and turns a [`GraphicBuffer`] into a [`Graphic`].
///
/// The renderer is the only thing that reaches a generator, and it is what makes
/// the generator's preconditions hold. Every path that changes the grid, the
/// size, or the generator itself hands the generator what it needs before the
/// next [`render`](Self::render).
///
/// It holds no channel and spawns no thread, so a test drives it directly;
/// [`start`] is what puts one on the graphic thread.
pub struct GraphicRenderer {
	generator: Box<dyn GraphicGenerator>,
	spectrum_history: VecDeque<Spectrum>,
	spectrum_params: Arc<SpectrumParams>,
	/// The size of the last buffer rendered, which the generator has been told.
	width: i32,
	height: i32,
}

impl GraphicRenderer {
	pub fn new() -> Self {
		GraphicRenderer {
			generator: Box::new(DefaultGraphicGenerator),
			spectrum_history: VecDeque::new(),
			spectrum_params: Arc::new(SpectrumParams::default()),
			width: 0,
			height: 0,
		}
	}

	fn update_spectrum(&mut self, spectrum: Spectrum) -> SpectrumBuffer {
		let max_history_len = self.generator.history_len();
		self.spectrum_history.truncate(max_history_len);
		if Arc::ptr_eq(spectrum.params(), &self.spectrum_params) {
			self.spectrum_history.push_front(spectrum);
		}
		if self.spectrum_history.len() > max_history_len {
			self.spectrum_history
				.pop_back()
				.expect("spectrum_history len is greater than 0")
				.into_buffer()
		} else {
			self.new_spectrum_buffer()
		}
	}

	fn new_spectrum_buffer(&self) -> SpectrumBuffer {
		SpectrumBuffer::new(self.spectrum_params.clone())
	}

	pub fn render(&mut self, buffer: GraphicBuffer) -> Result<Graphic, Error> {
		if buffer.width() != self.width || buffer.height() != self.height {
			self.width = buffer.width();
			self.height = buffer.height();
			self.generator.set_size(self.width, self.height);
		}
		self.generator.generate(buffer, &self.spectrum_history)
	}

	/// Applies `update` to the generator, then hands it the current grid and size.
	///
	/// `update` may replace the generator with one that has never been told
	/// either — `GraphicGeneratorConfig::update` swaps the box when the config
	/// names a different kind of generator — so re-establishing them is part of
	/// the same step. That is why the generator is reachable only through this,
	/// and why the closure is handed the `Box` rather than the trait object:
	/// replacing it is the point.
	pub fn update_generator(&mut self, update: impl FnOnce(&mut Box<dyn GraphicGenerator>)) {
		update(&mut self.generator);
		self.generator.set_params(&self.spectrum_params);
		self.generator.set_size(self.width, self.height);
	}

	pub fn spectrum_params(&self) -> &Arc<SpectrumParams> {
		&self.spectrum_params
	}

	pub fn set_spectrum_params(&mut self, params: SpectrumParams) {
		self.spectrum_params = Arc::new(params);
		self.spectrum_history.clear();
		self.generator.set_params(&self.spectrum_params);
	}
}

impl Default for GraphicRenderer {
	fn default() -> Self {
		Self::new()
	}
}

pub fn start(
	spectrum_input: mpsc::Receiver<Spectrum>,
	spectrum_output: mpsc::Sender<SpectrumBuffer>,
) -> Result<AsyncProcessor<GraphicRenderer>, Error> {
	start_with_thread_name("GraphicProcessor".into(), spectrum_input, spectrum_output)
}

pub fn start_with_thread_name(
	name: String,
	spectrum_input: mpsc::Receiver<Spectrum>,
	spectrum_output: mpsc::Sender<SpectrumBuffer>,
) -> Result<AsyncProcessor<GraphicRenderer>, Error> {
	let (exec_tx, exec_rx) = mpsc::channel(0);
	let _ = thread::Builder::new().name(name).spawn(move || {
		let mut processor = GraphicProcessor::new(exec_rx, spectrum_input, spectrum_output);
		executor::block_on(processor.process_loop())
	})?;
	Ok(AsyncProcessor::new(exec_tx))
}

struct GraphicProcessor {
	exec_rx: ExecReceiver<GraphicRenderer>,
	spectrum_input: mpsc::Receiver<Spectrum>,
	spectrum_output: mpsc::Sender<SpectrumBuffer>,
	current_buffer: Option<GraphicBuffer>,
	renderer: GraphicRenderer,
}

impl GraphicProcessor {
	fn new(
		exec_rx: ExecReceiver<GraphicRenderer>,
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
		match self
			.send_spectrum_buffer(self.renderer.new_spectrum_buffer())
			.await
		{
			Ok(true) => {}
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
				Ok(true) => {}
				Ok(false) => break,
				Err(err) => error!("error during graphic render processing: {}", err),
			}
		}
		debug!("Exiting graphic rendering thread");
	}

	fn handle_exec(
		&mut self,
		exec: Option<ExecCommand<GraphicRenderer>>,
	) -> Result<bool, GraphicProcessingError> {
		if let Some(exec) = exec {
			exec(&mut self.renderer);
			Ok(true)
		} else {
			debug!("graphic control channel disconnected, stopping graphic processing");
			Ok(false)
		}
	}

	async fn handle_new_spectrum(
		&mut self,
		spectrum: Option<Spectrum>,
	) -> Result<bool, GraphicProcessingError> {
		if let Some(spectrum) = spectrum {
			let buffer = self.renderer.update_spectrum(spectrum);
			self.send_spectrum_buffer(buffer).await
		} else {
			debug!("spectrum input channel closed, stopping graphic processing");
			Ok(false)
		}
	}

	async fn send_spectrum_buffer(
		&mut self,
		buffer: SpectrumBuffer,
	) -> Result<bool, GraphicProcessingError> {
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
