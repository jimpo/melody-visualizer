use futures::{channel::mpsc, executor, prelude::*, select};
use futures_timer::Delay;
use log::{debug, error};
use std::{
	fmt::Debug,
	thread,
	time::{Duration, Instant},
};

use crate::async_processor::{AsyncProcessor, ExecCommand, ExecReceiver};
use crate::error::Error;
use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumGenerator, TransformChain};

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From)]
enum SpectrumProcessingError {
	SendError(mpsc::SendError),
	#[display("received an unexpected buffer while one is already available")]
	ReceivedUnexpectedBuffer,
	#[display("skipping tick because no buffer is available")]
	NoBuffer,
}

#[derive(Debug)]
pub struct DefaultSpectrumGenerator;

impl SpectrumGenerator for DefaultSpectrumGenerator {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		buffer.fill(|_, _| ())
	}

	fn interval(&self) -> Duration {
		Duration::from_millis(1)
	}
}

/// The DSP chain: a generator and the transforms its spectra pass through.
///
/// Holds the state the chain needs and nothing else. [`SpectrumProcessor`] owns
/// one and drives it on the spectrum thread.
#[derive(Debug)]
pub struct SpectrumRenderer {
	generator: Box<dyn SpectrumGenerator>,
	transforms: TransformChain,
}

impl SpectrumRenderer {
	pub fn new() -> Self {
		SpectrumRenderer {
			generator: Box::new(DefaultSpectrumGenerator),
			transforms: TransformChain::default(),
		}
	}

	pub fn generator(&self) -> &dyn SpectrumGenerator {
		&*self.generator
	}

	pub fn generator_mut(&mut self) -> &mut dyn SpectrumGenerator {
		&mut *self.generator
	}

	pub fn set_generator(&mut self, generator: Box<dyn SpectrumGenerator>) {
		self.generator = generator;
	}

	pub fn transforms_mut(&mut self) -> &mut TransformChain {
		&mut self.transforms
	}
}

impl Default for SpectrumRenderer {
	fn default() -> Self {
		Self::new()
	}
}

struct SpectrumProcessor {
	renderer: SpectrumRenderer,
	current_buffer: Option<SpectrumBuffer>,
	exec_rx: ExecReceiver<SpectrumRenderer>,
	spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	spectrum_output: mpsc::Sender<Spectrum>,
	next_tick_time: Instant,
}

impl SpectrumProcessor {
	fn new(
		exec_rx: ExecReceiver<SpectrumRenderer>,
		spectrum_input: mpsc::Receiver<SpectrumBuffer>,
		spectrum_output: mpsc::Sender<Spectrum>,
	) -> Self {
		SpectrumProcessor {
			renderer: SpectrumRenderer::new(),
			current_buffer: None,
			exec_rx,
			spectrum_input,
			spectrum_output,
			next_tick_time: Instant::now(),
		}
	}

	async fn process_loop(&mut self) {
		debug!("Starting spectrum rendering thread");
		self.next_tick_time = Instant::now() + self.renderer.generator.interval();

		loop {
			let tick_delay = self
				.next_tick_time
				.saturating_duration_since(Instant::now());
			let result = select! {
				exec = self.exec_rx.next() => self.handle_exec(exec),
				new_buffer = self.spectrum_input.next() =>
					self.handle_new_buffer(new_buffer).await,
				_ = Delay::new(tick_delay).fuse() => self.handle_tick().await,
			};
			match result {
				Ok(true) => {}
				Ok(false) => break,
				Err(err) => error!("error during spectrum render processing: {}", err),
			}
		}
		debug!("Exiting spectrum rendering thread");
	}

	fn handle_exec(
		&mut self,
		exec: Option<ExecCommand<SpectrumRenderer>>,
	) -> Result<bool, SpectrumProcessingError> {
		if let Some(exec) = exec {
			exec(&mut self.renderer);
			Ok(true)
		} else {
			debug!("spectrum control channel disconnected, stopping spectrum processing");
			Ok(false)
		}
	}

	async fn handle_new_buffer(
		&mut self,
		new_buffer: Option<SpectrumBuffer>,
	) -> Result<bool, SpectrumProcessingError> {
		if let Some(new_buffer) = new_buffer {
			if self.current_buffer.is_some() {
				return Err(SpectrumProcessingError::ReceivedUnexpectedBuffer);
			} else {
				self.current_buffer = Some(new_buffer);
			}
			Ok(true)
		} else {
			debug!("spectrum input channel closed, stopping spectrum processing");
			Ok(false)
		}
	}

	async fn handle_tick(&mut self) -> Result<bool, SpectrumProcessingError> {
		self.next_tick_time += self.renderer.generator.interval();

		if let Some(buffer) = self.current_buffer.take() {
			let spectrum = self.render(buffer);
			if let Err(err) = self.spectrum_output.send(spectrum).await {
				return if err.is_disconnected() {
					debug!("spectrum output channel disconnected, stopping spectrum processing");
					Ok(false)
				} else {
					Err(err.into())
				};
			}
			Ok(true)
		} else {
			Err(SpectrumProcessingError::NoBuffer)
		}
	}

	fn render(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		self.renderer.transforms.set_params(buffer.params());
		let spectrum = self.renderer.generator.generate(buffer);
		self.renderer.transforms.apply(spectrum)
	}
}

pub fn start(
	spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	spectrum_output: mpsc::Sender<Spectrum>,
) -> Result<AsyncProcessor<SpectrumRenderer>, Error> {
	start_with_thread_name("SpectrumProcessor".into(), spectrum_input, spectrum_output)
}

pub fn start_with_thread_name(
	name: String,
	spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	spectrum_output: mpsc::Sender<Spectrum>,
) -> Result<AsyncProcessor<SpectrumRenderer>, Error> {
	let (exec_tx, exec_rx) = mpsc::channel(0);
	let _ = thread::Builder::new().name(name).spawn(move || {
		let mut processor = SpectrumProcessor::new(exec_rx, spectrum_input, spectrum_output);
		executor::block_on(processor.process_loop())
	})?;
	Ok(AsyncProcessor::new(exec_tx))
}
