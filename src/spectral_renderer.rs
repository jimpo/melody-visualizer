use futures::{prelude::*, executor};
use log::{warn, error};
use std::thread::{self, Thread};
use std::time::Duration;
use std::sync::mpsc;

use crate::error::Error;

enum SpectrumProcessingError {
	SpectrumReceivedDuplicateBuffer,
}

pub struct SpectrumBuffer {
}

pub struct Spectrum {

}

pub type SpectrumGenerator = dyn FnMut(SpectrumBuffer) -> Spectrum;
pub type SpectrumTransform = dyn FnMut(Spectrum) -> Spectrum;

pub struct SpectrumRenderer {

}

impl SpectrumRenderer {
	fn new() -> Self {}
	fn next_tick_interval() -> Duration {}
	fn render(spectrum: SpectrumBuffer) -> Result<Spectrum, SpectrumProcessingError> {}
	fn set_generator(generator: Box<SpectrumGenerator>) {}
	fn insert_transform(transform: Box<SpectrumTransform>, index: usize) {}
	fn remove_transform(index: usize) -> Box<SpectrumTransform> {}
}

// pub struct SpectrumProcessingPipeline {}


pub struct AsyncSpectrumRenderer {
	thread: Thread,
	control_tx: mpsc::Sender<SpectrumRendererCmd>,
}

enum SpectrumRendererCmd {
	SetGenerator(Box<SpectrumGenerator>),
	Stop,
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
		let (control_tx, control_rx) = mpsc::channel();
		let processing_thread = thread::Builder()
			.set_name(name)
			.spawn(move || {
				executor::block_on(process_loop(control_rx, spectrum_input, spectrum_output));
			})?;
		Ok(AsyncSpectrumRenderer {
			thread: processing_thread,
			control_tx,
		})
	}

	pub async fn set_generator(generator: Box<SpectrumGenerator>) {}
	pub async fn insert_transform(transform: Box<SpectrumTransform>, index: usize) {}
	pub async fn remove_transform(index: usize) -> Box<SpectrumTransform> {}
}

async fn process_loop(
	mut control_rx: mpsc::Receiver<SpectrumRendererCmd>,
	mut spectrum_input: mpsc::Receiver<SpectrumBuffer>,
	spectrum_output: mpsc::Sender<Spectrum>,
) {
	let mut renderer = SpectrumRenderer::new();
	let mut buffer = None;
	loop {
		let result = select! {
			cmd = control_rx.next() => handle_cmd(&mut renderer, cmd),
			new_buffer = spectrum_input =>
				handle_new_buffer(&mut renderer, &mut buffer, new_buffer, &spectrum_output),
		}.await;
		match result {
			Ok(true) => {},
			Ok(false) => break,
			Err(err) => error!("error during spectrum render processing: {}", err),
		}
	}
}

async fn handle_cmd(renderer: &mut SpectrumRenderer, cmd: SpectrumRendererCmd)
	-> Result<bool, SpectrumProcessingError>
{
	match cmd {
		SpectrumRendererCmd::SetGenerator(generator) =>
			renderer.set_generator(generator),
		SpectrumRendererCmd::Stop => Ok(false),
	}
}

async fn handle_new_buffer(
	renderer: &mut SpectrumRenderer,
	buffer: &mut Option<SpectrumBuffer>,
	new_buffer: SpectrumBuffer,
	spectrum_output: &mpsc::Sender<Spectrum>,
) -> Result<bool, SpectrumProcessingError> {
	if buffer.is_some() {
		return Err(SpectrumProcessingError::SpectrumReceivedDuplicateBuffer);
	} else {
		*buffer = Some(new_buffer);
	}
	// TODO: make this happen on a timer tick.
	if let Some(buffer) = buffer.take() {
		let spectrum = renderer.render(buffer);
		spectrum_output.send(spectrum).await?;
	} else {
		warn!("Spectrum renderer ticked and no buffer is available");
	}
	Ok(true)
}
