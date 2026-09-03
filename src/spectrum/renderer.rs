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

	/// Renders one spectrum: fill `buffer` from the generator, then run it
	/// through the transform chain.
	///
	/// Pure, deterministic arithmetic over the state the renderer holds. It
	/// needs no thread, no channel and no audio server, so the whole DSP chain
	/// can be driven from a test or a benchmark.
	pub fn render(&mut self, buffer: SpectrumBuffer) -> Spectrum {
		self.transforms.set_params(buffer.params());
		let spectrum = self.generator.generate(buffer);
		self.transforms.apply(spectrum)
	}
}

impl Default for SpectrumRenderer {
	fn default() -> Self {
		Self::new()
	}
}

/// Drives a [`SpectrumRenderer`] on the spectrum thread.
///
/// Owns the tick clock, the buffer channels and the control channel, and no DSP
/// logic of its own.
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
			let spectrum = self.renderer.render(buffer);
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

#[cfg(test)]
mod tests {
	use super::*;

	use crate::spectrum::TransformId;
	use crate::spectrum::transforms::{Diffuser, VolumeNormalizer, diffuser, volume_normalizer};
	use crate::test_support::spectrum_params;
	use crate::traits::Configurable;

	/// A generator that fills every spectrum with the ramp `1, 2, 3, …`.
	#[derive(Debug)]
	struct RampGenerator;

	impl SpectrumGenerator for RampGenerator {
		fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum {
			buffer.fill(|values, _params| {
				for (index, value) in values.iter_mut().enumerate() {
					*value = (index + 1) as f64;
				}
			})
		}

		fn interval(&self) -> Duration {
			Duration::from_millis(1)
		}
	}

	fn ramp_renderer() -> SpectrumRenderer {
		let mut renderer = SpectrumRenderer::new();
		renderer.set_generator(Box::new(RampGenerator));
		renderer
	}

	#[test]
	fn render_runs_the_generator_and_then_the_chain() {
		let mut renderer = ramp_renderer();
		renderer.transforms_mut().insert(
			0,
			TransformId(0),
			Box::new(VolumeNormalizer::new(volume_normalizer::Config {
				rate: 1.0,
			})),
		);

		let spectrum = renderer.render(SpectrumBuffer::new(spectrum_params(4)));

		assert_eq!(
			spectrum.values(),
			[0.25, 0.5, 0.75, 1.0],
			"the generated ramp comes out scaled by the chain's normalizer",
		);
	}

	#[test]
	fn render_hands_the_frequency_grid_to_the_chain() {
		let mut renderer = ramp_renderer();
		renderer.transforms_mut().insert(
			0,
			TransformId(0),
			// A zero-width diffuser is the identity, but it still convolves
			// through a buffer it sizes from the grid — so it produces a
			// spectrum at all only if the grid reached it.
			Box::new(Diffuser::new(diffuser::Config { width: 0.0 })),
		);

		let spectrum = renderer.render(SpectrumBuffer::new(spectrum_params(4)));
		assert_eq!(spectrum.values(), [1.0, 2.0, 3.0, 4.0]);

		let spectrum = renderer.render(SpectrumBuffer::new(spectrum_params(6)));
		assert_eq!(
			spectrum.values(),
			[1.0, 2.0, 3.0, 4.0, 5.0, 6.0],
			"a grid the renderer has not seen before reaches the chain",
		);
	}
}
