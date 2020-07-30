use futures::{prelude::*, channel::mpsc, executor, select};
use futures_timer::Delay;
use log::{debug, error};
use std::fmt::Debug;
use std::thread;
use std::time::{Duration, Instant};

use crate::async_processor::AsyncProcessor;
use crate::error::Error;
use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumGenerator, SpectrumTransform};

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From)]
enum SpectrumProcessingError {
	SendError(mpsc::SendError),
	#[display(fmt = "received an unexpected buffer while one is already available")]
	ReceivedUnexpectedBuffer,
	#[display(fmt = "skipping tick because no buffer is available")]
	NoBuffer,
}

#[derive(Debug)]
pub struct DefaultSpectrumGenerator;

impl SpectrumGenerator for DefaultSpectrumGenerator {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		debug!("generating spectrum");
		buffer.fill(|_, _| ())
	}

	fn interval(&self) -> Duration {
		Duration::from_millis(1)
	}
}

pub struct SpectrumRenderer {
	generator: Box<dyn SpectrumGenerator>,
	transforms: Vec<Box<dyn SpectrumTransform>>,
}

impl SpectrumRenderer {
	fn new() -> Self {
		SpectrumRenderer {
			generator: Box::new(DefaultSpectrumGenerator),
			transforms: Vec::new(),
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

	pub fn transforms_mut(&mut self) -> &mut Vec<Box<dyn SpectrumTransform>> {
		&mut self.transforms
	}
}

struct SpectrumProcessor {
	renderer: SpectrumRenderer,
	current_buffer: Option<SpectrumBuffer>,
	exec_rx: mpsc::Receiver<Box<dyn FnOnce(&mut SpectrumRenderer) + Send>>,
	spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	spectrum_output: mpsc::Sender<Spectrum>,
	next_tick_time: Instant,
}

impl SpectrumProcessor {
	fn new(
		exec_rx: mpsc::Receiver<Box<dyn FnOnce(&mut SpectrumRenderer) + Send>>,
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
			let tick_delay = self.next_tick_time.saturating_duration_since(Instant::now());
			let result = select! {
				exec = self.exec_rx.next() => self.handle_exec(exec),
				new_buffer = self.spectrum_input.next() =>
					self.handle_new_buffer(new_buffer).await,
				_ = Delay::new(tick_delay).fuse() => self.handle_tick().await,
			};
			match result {
				Ok(true) => {},
				Ok(false) => break,
				Err(err) => error!("error during spectrum render processing: {}", err),
			}
		}
		debug!("Exiting spectrum rendering thread");
	}

	fn handle_exec(&mut self, exec: Option<Box<dyn FnOnce(&mut SpectrumRenderer) + Send>>)
		-> Result<bool, SpectrumProcessingError>
	{
		if let Some(exec) = exec {
			exec(&mut self.renderer);
			Ok(true)
		} else {
			debug!("spectrum control channel disconnected, stopping spectrum processing");
			Ok(false)
		}
	}

	async fn handle_new_buffer(&mut self, new_buffer: Option<SpectrumBuffer>)
		-> Result<bool, SpectrumProcessingError>
	{
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
			let spectrum = self.render(buffer)?;
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

	fn render(&mut self, buffer: SpectrumBuffer) -> Result<Spectrum, SpectrumProcessingError> {
		let initial_spectrum = self.renderer.generator.generate(buffer);
		let final_spectrum = self.renderer.transforms.iter_mut()
			.fold(initial_spectrum, |spectrum, transform| transform.transform(spectrum));
		Ok(final_spectrum)
	}
}

pub fn start(
	spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	spectrum_output: mpsc::Sender<Spectrum>,
) -> Result<AsyncProcessor<SpectrumRenderer>, Error>
{
	start_with_thread_name("SpectrumProcessor".into(), spectrum_input, spectrum_output)
}

pub fn start_with_thread_name(
	name: String,
	spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	spectrum_output: mpsc::Sender<Spectrum>,
) -> Result<AsyncProcessor<SpectrumRenderer>, Error>
{
	let (exec_tx, exec_rx) = mpsc::channel(0);
	let mut processor = SpectrumProcessor::new(exec_rx, spectrum_input, spectrum_output);
	let _ = thread::Builder::new()
		.name(name)
		.spawn(move || executor::block_on(processor.process_loop()))?;
	Ok(AsyncProcessor::new(exec_tx))
}