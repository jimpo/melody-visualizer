use futures::{prelude::*, channel::{mpsc, oneshot}, executor, select};
use log::{debug, warn, error};
use std::any::Any;
use std::fmt::Debug;
use std::thread;

use crate::async_processor::AsyncProcessor;
use crate::error::Error;
use crate::spectrum::{Spectrum, SpectrumBuffer};

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From)]
enum SpectrumProcessingError {
	SendError(mpsc::SendError),
	#[display(fmt = "received an unexpected buffer while one is already available")]
	ReceivedUnexpectedBuffer,
}

pub trait SpectrumGenerator: Debug + Send {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum;
}

pub trait SpectrumTransform: Debug + Send {
	fn transform(&mut self, spectrum: Spectrum) -> Spectrum;
}

#[derive(Debug)]
pub struct DefaultSpectrumGenerator;

impl SpectrumGenerator for DefaultSpectrumGenerator {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		Spectrum::default()
	}
}

struct SpectrumProcessor {
	generator: Box<dyn SpectrumGenerator>,
	transforms: Vec<Box<dyn SpectrumTransform>>,
	current_buffer: Option<SpectrumBuffer>,
	control_rx: mpsc::Receiver<(SpectrumRendererCmd, oneshot::Sender<Box<dyn Any + Send>>)>,
	spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	spectrum_output: mpsc::Sender<Spectrum>,
}

impl SpectrumProcessor {
	fn new(
		control_rx: mpsc::Receiver<(SpectrumRendererCmd, oneshot::Sender<Box<dyn Any + Send>>)>,
		spectrum_input: mpsc::Receiver<SpectrumBuffer>,
		spectrum_output: mpsc::Sender<Spectrum>,
	) -> Self {
		SpectrumProcessor {
			generator: Box::new(DefaultSpectrumGenerator),
			transforms: Vec::new(),
			current_buffer: None,
			control_rx,
			spectrum_input,
			spectrum_output,
		}
	}

	async fn process_loop(&mut self) {
		debug!("Starting spectrum rendering thread");
		loop {
			let result = select! {
				cmd = self.control_rx.next() => self.handle_cmd(cmd).await,
				new_buffer = self.spectrum_input.next() =>
					self.handle_new_buffer(new_buffer).await,
			};
			match result {
				Ok(true) => {},
				Ok(false) => break,
				Err(err) => error!("error during spectrum render processing: {}", err),
			}
		}
		debug!("Exiting spectrum rendering thread");
	}

	async fn handle_cmd(
		&mut self,
		request: Option<(SpectrumRendererCmd, oneshot::Sender<Box<dyn Any + Send>>)>
	) -> Result<bool, SpectrumProcessingError>
	{
		if let Some((cmd, reply_tx)) = request {
			debug!("spectrum rendering thread received command: {:?}", cmd);
			let result = match cmd {
				SpectrumRendererCmd::SetGenerator(generator) => {
					self.generator = generator;
					Box::new(())
				}
			};
			if let Err(err) = reply_tx.send(result) {
				debug!("RPC response channel disconnected");
			}
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
			// TODO: make this happen on a timer tick.
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
			} else {
				warn!("Spectrum renderer ticked and no buffer is available");
			}
			Ok(true)
		} else {
			debug!("spectrum input channel closed, stopping spectrum processing");
			Ok(false)
		}
	}

	fn render(&mut self, buffer: SpectrumBuffer) -> Result<Spectrum, SpectrumProcessingError> {
		let initial_spectrum = self.generator.generate(buffer);
		let final_spectrum = self.transforms.iter_mut()
			.fold(initial_spectrum, |spectrum, transform| transform.transform(spectrum));
		Ok(final_spectrum)
	}
}

#[derive(Debug)]
pub enum SpectrumRendererCmd {
	SetGenerator(Box<dyn SpectrumGenerator>),
}

pub fn start(
	spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	spectrum_output: mpsc::Sender<Spectrum>,
) -> Result<AsyncProcessor<SpectrumRendererCmd>, Error>
{
	start_with_thread_name("SpectrumProcessor".into(), spectrum_input, spectrum_output)
}

pub fn start_with_thread_name(
	name: String,
	spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	spectrum_output: mpsc::Sender<Spectrum>,
) -> Result<AsyncProcessor<SpectrumRendererCmd>, Error>
{
	let (control_tx, control_rx) = mpsc::channel(0);
	let mut processor = SpectrumProcessor::new(control_rx, spectrum_input, spectrum_output);
	let _ = thread::Builder::new()
		.name(name)
		.spawn(move || executor::block_on(processor.process_loop()))?;
	Ok(AsyncProcessor::new(control_tx))
}