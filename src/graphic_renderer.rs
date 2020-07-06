use futures::channel::mpsc;
use std::thread::{self, Thread};

use crate::spectral_renderer::{Spectrum, SpectrumBuffer};
use crate::error::Error;

struct GraphicRenderer {

}

impl GraphicRenderer {
	fn new() -> Self {}
	fn render(surface: &cairo::ImageSurface) {}
}

pub struct AsyncGraphicRenderer {
	thread: Thread,
	sender: mpsc::Sender<GraphicRendererCmd>,
}

enum GraphicRendererCmd {
	Render(cairo::ImageSurface),
	Stop,
}

impl AsyncGraphicRenderer {
	pub fn new(
		graphic_input: mpsc::Receiver<cairo::ImageSurface>,
		graphic_output: mpsc::Sender<cairo::ImageSurface>,
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
		graphic_input: mpsc::Receiver<cairo::ImageSurface>,
		graphic_output: mpsc::Sender<cairo::ImageSurface>,
		spectrum_input: mpsc::Receiver<Spectrum>,
		spectrum_output: mpsc::Sender<SpectrumBuffer>,
	) -> Result<Self, Error> {
		let (sender, receiver) = mpsc::channel(0);
		let processing_thread = thread::Builder()
			.set_name(name)
			.spawn(move || process(receiver))?;
		Ok(AsyncGraphicRenderer {
			thread: processing_thread,
			sender,
		})
	}

	// async fn update params
}

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

