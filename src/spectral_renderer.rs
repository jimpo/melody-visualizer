use futures::{prelude::*, channel::mpsc, executor, select};
use log::{debug, warn, error};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::error::Error;

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From)]
enum SpectrumProcessingError {
	SendError(mpsc::SendError),
	SpectrumReceivedDuplicateBuffer,
}

pub struct SpectrumBuffer {
}

impl Default for SpectrumBuffer {
	fn default() -> Self {
		SpectrumBuffer {}
	}
}

pub struct Spectrum {
}

impl Spectrum {
	pub fn into_buffer(self) -> SpectrumBuffer {
		SpectrumBuffer {}
	}
}

pub type SpectrumGenerator = dyn FnMut(SpectrumBuffer) -> Spectrum;
pub type SpectrumTransform = dyn FnMut(Spectrum) -> Spectrum;

pub struct SpectrumRenderer {

}

impl SpectrumRenderer {
	fn new() -> Self {}
	fn next_tick_interval(&self) -> Duration {}
	fn render(&mut self, spectrum: SpectrumBuffer) -> Result<Spectrum, SpectrumProcessingError> {}
	fn set_generator(&mut self, generator: Box<SpectrumGenerator>) {}
	fn insert_transform(&mut self, transform: Box<SpectrumTransform>, index: usize) {}
	fn remove_transform(&mut self, index: usize) -> Box<SpectrumTransform> {}
}

// pub struct SpectrumProcessingPipeline {}


pub struct AsyncSpectrumRenderer {
	thread: JoinHandle<()>,
	control_tx: mpsc::Sender<SpectrumRendererCmd>,
}

enum SpectrumRendererCmd {
	SetGenerator(Box<SpectrumGenerator>),
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
		let processing_thread = thread::Builder::new()
			.name(name)
			.spawn(move || {
				executor::block_on(process_loop(control_rx, spectrum_input, spectrum_output));
			})?;
		Ok(AsyncSpectrumRenderer {
			thread: processing_thread,
			control_tx,
		})
	}

	// pub async fn set_generator(generator: Box<SpectrumGenerator>) {}
	// pub async fn insert_transform(transform: Box<SpectrumTransform>, index: usize) {}
	// pub async fn remove_transform(index: usize) -> Box<SpectrumTransform> {}
}

async fn process_loop(
	mut control_rx: mpsc::Receiver<SpectrumRendererCmd>,
	mut spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	mut spectrum_output: mpsc::Sender<Spectrum>,
) {
	let mut renderer = SpectrumRenderer::new();
	let mut buffer = None;
	loop {
		let result = select! {
			cmd = control_rx.next().fuse() => handle_cmd(&mut renderer, cmd).await,
			new_buffer = spectrum_input.next().fuse() => {
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
}

async fn handle_cmd(renderer: &mut SpectrumRenderer, cmd: Option<SpectrumRendererCmd>)
	-> Result<bool, SpectrumProcessingError>
{
	match cmd {
		Some(SpectrumRendererCmd::SetGenerator(generator)) => {
			renderer.set_generator(generator);
			Ok(true)
		}
		None => Ok(false),
	}
}

async fn handle_new_buffer(
	renderer: &mut SpectrumRenderer,
	buffer: &mut Option<SpectrumBuffer>,
	new_buffer: Option<SpectrumBuffer>,
	spectrum_output: &mut mpsc::Sender<Spectrum>,
) -> Result<bool, SpectrumProcessingError> {
	if let Some(new_buffer) = new_buffer {
		if buffer.is_some() {
			return Err(SpectrumProcessingError::SpectrumReceivedDuplicateBuffer);
		} else {
			*buffer = Some(new_buffer);
		}
		// TODO: make this happen on a timer tick.
		if let Some(buffer) = buffer.take() {
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
