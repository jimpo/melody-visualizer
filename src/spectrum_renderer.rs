use futures::{prelude::*, channel::{mpsc, oneshot}, executor, select};
use log::{debug, warn, error};
use std::any::Any;
use std::fmt::Debug;
use std::thread;

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

pub struct SpectrumRenderer {
	generator: Box<dyn SpectrumGenerator>,
	transforms: Vec<Box<dyn SpectrumTransform>>,
}

#[derive(Debug)]
pub struct DefaultSpectrumGenerator;

impl SpectrumGenerator for DefaultSpectrumGenerator {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		Spectrum::default()
	}
}

impl SpectrumRenderer {
	fn new() -> Self {
		SpectrumRenderer {
			generator: Box::new(DefaultSpectrumGenerator),
			transforms: Vec::new(),
		}
	}

	// fn next_tick_interval(&self) -> Duration {}

	fn render(&mut self, buffer: SpectrumBuffer) -> Result<Spectrum, SpectrumProcessingError> {
		let initial_spectrum = self.generator.generate(buffer);
		let final_spectrum = self.transforms.iter_mut()
			.fold(initial_spectrum, |spectrum, transform| transform.transform(spectrum));
		Ok(final_spectrum)
	}

	fn set_generator(&mut self, generator: Box<dyn SpectrumGenerator>) {
		self.generator = generator;
	}

	// fn insert_transform(&mut self, transform: Box<SpectrumTransform>, index: usize) {}
	// fn remove_transform(&mut self, index: usize) -> Box<SpectrumTransform> {}
}

// pub struct SpectrumProcessingPipeline {}


#[derive(Debug)]
pub enum SpectrumRendererCmd {
	SetGenerator(Box<dyn SpectrumGenerator>),
}

#[derive(Clone)]
pub struct AsyncSpectrumRenderer {
	control_tx: mpsc::Sender<(SpectrumRendererCmd, oneshot::Sender<Box<dyn Any + Send>>)>,
}

impl AsyncSpectrumRenderer {
	pub fn new(
		spectrum_input: mpsc::Receiver<SpectrumBuffer>,
		spectrum_output: mpsc::Sender<Spectrum>
	) -> Result<Self, Error> {
		Self::with_thread_name("AsyncSpectrumRenderer".into(), spectrum_input, spectrum_output)
	}

	pub fn with_thread_name(
		name: String,
		spectrum_input: mpsc::Receiver<SpectrumBuffer>,
		spectrum_output: mpsc::Sender<Spectrum>,
	) -> Result<Self, Error> {
		let (control_tx, control_rx) = mpsc::channel(0);
		let _ = thread::Builder::new()
			.name(name)
			.spawn(move || {
				executor::block_on(process_loop(control_rx, spectrum_input, spectrum_output));
			})?;
		Ok(AsyncSpectrumRenderer {
			control_tx,
		})
	}

	pub async fn call<R: Any + Send>(&mut self, cmd: SpectrumRendererCmd)
		-> Result<R, Error>
	{
		let (reply_tx, reply_rx) = oneshot::channel();
		self.control_tx.send((cmd, reply_tx)).await
			.map_err(Error::ProcessingControlError)?;

		let reply_untyped = reply_rx.await
			.map_err(|_| Error::AsyncCallFailure)?;
		let reply = reply_untyped.downcast()
			.map_err(|_| Error::AsyncCallFailure)?;
		Ok(*reply)
	}

	pub async fn stop(&mut self) -> Result<(), Error> {
		if let Err(err) = self.control_tx.close().await {
			if !err.is_disconnected() {
				return Err(Error::ProcessingControlError(err));
			}
		}
		Ok(())
	}

	// pub async fn set_generator(generator: Box<SpectrumGenerator>) {}
	// pub async fn insert_transform(transform: Box<SpectrumTransform>, index: usize) {}
	// pub async fn remove_transform(index: usize) -> Box<SpectrumTransform> {}
}

async fn process_loop(
	mut control_rx: mpsc::Receiver<(SpectrumRendererCmd, oneshot::Sender<Box<dyn Any + Send>>)>,
	mut spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	mut spectrum_output: mpsc::Sender<Spectrum>,
) {
	debug!("Starting spectrum rendering thread");
	let mut renderer = SpectrumRenderer::new();
	let mut buffer = None;
	loop {
		let result = select! {
			cmd = control_rx.next() => handle_cmd(&mut renderer, cmd).await,
			new_buffer = spectrum_input.next() => {
				handle_new_buffer(&mut renderer, &mut buffer, new_buffer, &mut spectrum_output)
					.await
			}
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
	renderer: &mut SpectrumRenderer,
	request: Option<(SpectrumRendererCmd, oneshot::Sender<Box<dyn Any + Send>>)>,
) -> Result<bool, SpectrumProcessingError>
{
	if let Some((cmd, reply_tx)) = request {
		debug!("spectrum rendering thread received command: {:?}", cmd);
		let result = match cmd {
			SpectrumRendererCmd::SetGenerator(generator) => {
				renderer.set_generator(generator);
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

async fn handle_new_buffer(
	renderer: &mut SpectrumRenderer,
	current_buffer: &mut Option<SpectrumBuffer>,
	new_buffer: Option<SpectrumBuffer>,
	spectrum_output: &mut mpsc::Sender<Spectrum>,
) -> Result<bool, SpectrumProcessingError> {
	if let Some(new_buffer) = new_buffer {
		if current_buffer.is_some() {
			return Err(SpectrumProcessingError::ReceivedUnexpectedBuffer);
		} else {
			*current_buffer = Some(new_buffer);
		}
		// TODO: make this happen on a timer tick.
		if let Some(buffer) = current_buffer.take() {
			let spectrum = renderer.render(buffer)?;
			if let Err(err) = spectrum_output.send(spectrum).await {
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
