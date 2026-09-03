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

	/// Takes `spectrum` into the history and returns a buffer for the spectrum
	/// thread to fill next.
	///
	/// The returned buffer is the one the history evicts, which is what keeps the
	/// steady state free of allocation (ARCHITECTURE.md § 3).
	fn update_spectrum(&mut self, spectrum: Spectrum) -> SpectrumBuffer {
		let max_history_len = self.generator.history_len();
		self.spectrum_history.truncate(max_history_len);

		if !Arc::ptr_eq(spectrum.params(), &self.spectrum_params) {
			// The spectrum was built on a grid this renderer has since replaced,
			// so it is no use as history. Its allocation still is: hand it
			// straight back on the current grid. The window is short — it closes
			// once the spectrum thread has been reconfigured too — but it opens on
			// every frequency-range change, which is every drag of a slider.
			return spectrum.into_buffer().regrid(self.spectrum_params.clone());
		}

		self.spectrum_history.push_front(spectrum);
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

#[cfg(test)]
mod tests {
	use super::*;

	use std::sync::Mutex;

	/// What a [`Recorder`] was asked to do.
	#[derive(Debug, Clone, Copy, PartialEq, Eq)]
	enum Call {
		Generate,
		SetParams,
		SetSize(i32, i32),
	}

	/// A generator that records its calls into a log shared with the test, and
	/// draws nothing.
	#[derive(Debug)]
	struct Recorder {
		log: Arc<Mutex<Vec<Call>>>,
		history_len: usize,
	}

	impl GraphicGenerator for Recorder {
		fn generate(
			&mut self,
			buffer: GraphicBuffer,
			_spectrum_history: &VecDeque<Spectrum>,
		) -> Result<Graphic, Error> {
			self.log.lock().unwrap().push(Call::Generate);
			buffer.draw(|_ctx| Ok(()))
		}

		fn set_params(&mut self, _params: &Arc<SpectrumParams>) {
			self.log.lock().unwrap().push(Call::SetParams);
		}

		fn set_size(&mut self, width: i32, height: i32) {
			self.log.lock().unwrap().push(Call::SetSize(width, height));
		}

		fn history_len(&self) -> usize {
			self.history_len
		}

		fn as_any_mut(&mut self) -> &mut dyn Any {
			self
		}
	}

	/// A renderer driving a recorder that keeps `history_len` spectra, and the
	/// log they share.
	fn recording_renderer(history_len: usize) -> (GraphicRenderer, Arc<Mutex<Vec<Call>>>) {
		let log = Arc::new(Mutex::new(Vec::new()));
		let mut renderer = GraphicRenderer::new();
		renderer.update_generator(|generator| {
			*generator = Box::new(Recorder {
				log: log.clone(),
				history_len,
			});
		});
		log.lock().unwrap().clear();
		(renderer, log)
	}

	/// Where a buffer's values live, and the buffer back again.
	///
	/// The address is what tells a recycled allocation from a fresh one.
	fn allocation(buffer: SpectrumBuffer) -> (usize, SpectrumBuffer) {
		let spectrum = buffer.fill(|_values, _params| {});
		let address = spectrum.values().as_ptr() as usize;
		(address, spectrum.into_buffer())
	}

	#[test]
	fn a_spectrum_on_a_stale_grid_hands_its_allocation_straight_back() {
		let mut renderer = GraphicRenderer::new();
		renderer.set_spectrum_params(SpectrumParams::exp_spaced(64, 200.0, 20000.0));

		// A spectrum from before the renderer's grid changed. It is a distinct
		// `Arc`, which is how the mismatch is detected, and the same length, so
		// the allocation is reusable without growing.
		let stale = Arc::new(SpectrumParams::exp_spaced(64, 200.0, 20000.0));
		let (incoming, buffer) = allocation(SpectrumBuffer::new(stale));

		let returned = renderer.update_spectrum(buffer.fill(|_values, _params| {}));

		assert!(
			renderer.spectrum_history.is_empty(),
			"a spectrum on a grid the renderer has left is no use as history",
		);
		assert!(
			Arc::ptr_eq(returned.params(), renderer.spectrum_params()),
			"the buffer comes back on the grid the spectrum thread should fill next",
		);
		assert_eq!(
			allocation(returned).0,
			incoming,
			"the steady state allocates nothing, reconfiguration included",
		);
	}

	#[test]
	fn the_evicted_spectrum_is_the_buffer_handed_back() {
		let (mut renderer, _log) = recording_renderer(2);
		let params = renderer.spectrum_params().clone();

		// Filling the history costs one buffer per spectrum: there is nothing to
		// evict yet.
		for _ in 0..2 {
			renderer.update_spectrum(SpectrumBuffer::new(params.clone()).fill(|_v, _p| {}));
		}
		assert_eq!(renderer.spectrum_history.len(), 2);

		let (oldest, buffer) = allocation(SpectrumBuffer::new(params.clone()));
		renderer.update_spectrum(buffer.fill(|_v, _p| {}));
		let returned = renderer
			.update_spectrum(SpectrumBuffer::new(params.clone()).fill(|_values, _params| {}));

		assert_eq!(
			renderer.spectrum_history.len(),
			2,
			"the generator asked for two spectra, so the third pushes one out",
		);
		assert_eq!(
			allocation(returned).0,
			oldest,
			"the spectrum the history evicts is the buffer the spectrum thread gets",
		);
	}

	#[test]
	fn the_generator_is_told_the_grid_and_the_size_it_will_be_drawing_on() {
		let (mut renderer, log) = recording_renderer(1);

		renderer.render(GraphicBuffer::new(64, 32)).unwrap();
		renderer.render(GraphicBuffer::new(64, 32)).unwrap();
		renderer.set_spectrum_params(SpectrumParams::exp_spaced(8, 200.0, 20000.0));
		renderer.render(GraphicBuffer::new(128, 64)).unwrap();

		assert_eq!(
			*log.lock().unwrap(),
			[
				Call::SetSize(64, 32),
				Call::Generate,
				Call::Generate,
				Call::SetParams,
				Call::SetSize(128, 64),
				Call::Generate,
			],
			"the hooks fire on a change and only on a change, always before the \
			 frame that depends on them",
		);
	}

	#[test]
	fn a_generator_joining_a_renderer_is_handed_the_grid_and_the_size() {
		let (mut renderer, _log) = recording_renderer(1);
		renderer.render(GraphicBuffer::new(64, 32)).unwrap();
		renderer.set_spectrum_params(SpectrumParams::exp_spaced(8, 200.0, 20000.0));

		let log = Arc::new(Mutex::new(Vec::new()));
		renderer.update_generator(|generator| {
			*generator = Box::new(Recorder {
				log: log.clone(),
				history_len: 1,
			});
		});

		assert_eq!(
			*log.lock().unwrap(),
			[Call::SetParams, Call::SetSize(64, 32)],
			"a replacement generator has been told neither, so the renderer tells it",
		);
	}
}
