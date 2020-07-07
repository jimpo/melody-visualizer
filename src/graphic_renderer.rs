use futures::{prelude::*, channel::mpsc, executor, select};
use log::{debug, error};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use crate::graphic::{Graphic, GraphicBuffer};
use crate::spectrum::{Spectrum, SpectrumBuffer};
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

#[derive(Debug, derive_more::Display, derive_more::Error, derive_more::From)]
enum GraphicProcessingError {
	SendError(mpsc::SendError),
	Other(Error),
}


struct GraphicRenderer {
}

impl GraphicRenderer {
	fn new() -> Self {
		GraphicRenderer {}
	}

	fn render(&mut self, buffer: GraphicBuffer) -> Result<Graphic, Error> {
		let x_max = buffer.width();
		let y_max = buffer.height();

		buffer.draw(|ctx| {
			// Dummy routine. Make the whole area red.
			ctx.set_source_rgb(255.0, 0.0, 0.0);
			ctx.rectangle(0.0, 0.0, x_max as f64, y_max as f64);
			ctx.fill();
			Ok(())
		})
	}

	fn update_spectrum(&mut self, spectrum: Spectrum) -> SpectrumBuffer {
		spectrum.into_buffer()
	}
}

pub struct AsyncGraphicRenderer {
	thread: Option<JoinHandle<()>>,
	control_tx: mpsc::Sender<GraphicRendererCmd>,
}

#[derive(Debug)]
enum GraphicRendererCmd {
}

impl AsyncGraphicRenderer {
	pub fn new(
		graphic_input: mpsc::Receiver<GraphicBuffer>,
		graphic_output: mpsc::Sender<Graphic>,
		spectrum_input: mpsc::Receiver<Spectrum>,
		spectrum_output: mpsc::Sender<SpectrumBuffer>,
	) -> Result<Self, Error>	{
		Self::with_thread_name(
			"AsyncGraphicRenderer".into(),
			graphic_input,
			graphic_output,
			spectrum_input,
			spectrum_output,
		)
	}

	pub fn with_thread_name(
		name: String,
		graphic_input: mpsc::Receiver<GraphicBuffer>,
		graphic_output: mpsc::Sender<Graphic>,
		spectrum_input: mpsc::Receiver<Spectrum>,
		spectrum_output: mpsc::Sender<SpectrumBuffer>,
	) -> Result<Self, Error> {
		let (control_tx, control_rx) = mpsc::channel(0);
		let processing_thread = thread::Builder::new()
			.name(name)
			.spawn(move || {
				executor::block_on(process_loop(
					control_rx,
					graphic_input,
					graphic_output,
					spectrum_input,
					spectrum_output,
				));
			})?;
		Ok(AsyncGraphicRenderer {
			thread: Some(processing_thread),
			control_tx,
		})
	}

	// async fn update params
	pub async fn stop(&mut self) -> Result<(), Error> {
		if let Err(err) = self.control_tx.close().await {
			if !err.is_disconnected() {
				return Err(Error::ProcessingControlError(err));
			}
		}
		Ok(())
	}
}

async fn process_loop(
	mut control_rx: mpsc::Receiver<GraphicRendererCmd>,
	mut graphic_input: mpsc::Receiver<GraphicBuffer>,
	mut graphic_output: mpsc::Sender<Graphic>,
	mut spectrum_input: mpsc::Receiver<Spectrum>,
	mut spectrum_output: mpsc::Sender<SpectrumBuffer>,
) {
	debug!("Starting graphic rendering thread");
	let mut renderer = GraphicRenderer::new();
	loop {
		let result = select! {
			cmd = control_rx.next() => handle_cmd(&mut renderer, cmd).await,
			new_buffer = graphic_input.next() =>
				handle_new_buffer(&mut renderer, new_buffer, &mut graphic_output).await,
			new_spectrum = spectrum_input.next() =>
				handle_new_spectrum(&mut renderer, new_spectrum, &mut spectrum_output).await,
		};
		match result {
			Ok(true) => {},
			Ok(false) => break,
			Err(err) => error!("error during graphic render processing: {}", err),
		}
	}
	debug!("Exiting graphic rendering thread");
}

async fn handle_cmd(renderer: &mut GraphicRenderer, cmd: Option<GraphicRendererCmd>)
	-> Result<bool, GraphicProcessingError>
{
	debug!("graphic rendering thread received command: {:?}", cmd);
	match cmd {
		Some(_) => Ok(true),
		None => Ok(false),
	}
}

async fn handle_new_spectrum(
	renderer: &mut GraphicRenderer,
	spectrum: Option<Spectrum>,
	spectrum_output: &mut mpsc::Sender<SpectrumBuffer>,
) -> Result<bool, GraphicProcessingError>
{
	if let Some(spectrum) = spectrum {
		let buffer = renderer.update_spectrum(spectrum);
		if let Err(err) = spectrum_output.send(buffer).await {
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

async fn handle_new_buffer(
	renderer: &mut GraphicRenderer,
	buffer: Option<GraphicBuffer>,
	graphic_output: &mut mpsc::Sender<Graphic>,
) -> Result<bool, GraphicProcessingError>
{
	if let Some(buffer) = buffer {
		let graphic = renderer.render(buffer)?;
		thread::sleep(Duration::from_millis(40));
		if let Err(err) = graphic_output.send(graphic).await {
			return if err.is_disconnected() {
				debug!("graphic output channel disconnected, stopping graphic processing");
				Ok(false)
			} else {
				Err(err.into())
			};
		}
		Ok(true)
	} else {
		debug!("graphic input channel closed, stopping graphic processing");
		Ok(false)
	}
}
