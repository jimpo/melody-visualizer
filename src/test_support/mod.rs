//! Shared test-only helpers.
//!
//! Compiled for the crate's own tests, and for dependents that enable the
//! `testing` feature.

pub mod jackd;

use futures::{channel::mpsc, prelude::*};
use glib::MainLoop;
use std::{f64::consts::TAU, sync::Arc, time::Instant};

use crate::app::config::{Config, SpectrumGeneratorConfig};
use crate::audio::SampleReader;
use crate::audio::ring::sample_ring;
use crate::graphic::renderer::GraphicRenderer;
use crate::spectrum::generators::audio::AudioSpectrumGenerator;
use crate::spectrum::renderer::SpectrumRenderer;
use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumParams};

/// The default frequency range, sampled at `samples` exponentially spaced bins.
///
/// # Preconditions
/// - `samples >= 2`
pub fn spectrum_params(samples: usize) -> Arc<SpectrumParams> {
	Arc::new(SpectrumParams::exp_spaced(samples, 200.0, 20000.0))
}

/// A spectrum holding `values`, on the grid [`spectrum_params`] builds for them.
///
/// # Preconditions
/// - `values.len() >= 2`
pub fn spectrum(values: &[f64]) -> Spectrum {
	SpectrumBuffer::new(spectrum_params(values.len()))
		.fill(|data, _params| data.copy_from_slice(values))
}

/// A sum of sine waves, sampled at `sample_rate`.
///
/// Each component is a `(frequency in Hz, amplitude)` pair. The result feeds
/// [`sample_reader`], which is how the DSP is driven with a known signal.
pub fn sine_wave(components: &[(f64, f64)], sample_rate: u32, samples: usize) -> Vec<f32> {
	(0..samples)
		.map(|index| {
			let time = index as f64 / sample_rate as f64;
			components
				.iter()
				.map(|(frequency, amplitude)| amplitude * (TAU * frequency * time).sin())
				.sum::<f64>() as f32
		})
		.collect()
}

/// A [`SampleReader`] holding `samples`, with no JACK server behind it.
///
/// Allocating a ring buffer is a plain userspace operation, so this is the seam
/// the generator is fed deterministic audio through. The reader peeks rather
/// than consumes, so the same samples serve every call to
/// [`generate`](crate::spectrum::SpectrumGenerator::generate).
pub fn sample_reader(samples: &[f32]) -> SampleReader {
	// The ring rounds its size up to a power of two and keeps one byte free, so
	// ask for more than the samples strictly need.
	let size = (samples.len() + 1) * size_of::<f32>() * 2;
	let (mut writer, reader) =
		sample_ring(size).expect("allocating a ring buffer needs no JACK server");
	writer.write_samples(samples);
	assert_eq!(
		reader.overruns(),
		0,
		"the ring was sized to hold every sample",
	);
	reader
}

/// Run async test code inside a glib main loop.
///
/// Several components (the `PubSub` event bus, the JACK notification handler)
/// only deliver their effects when a glib main loop is pumping. This driver runs
/// such a loop and hands the test an async context that can *yield* control back
/// to it: awaiting `yield_rx.next()` resumes the test once every task higher than
/// `Priority::LOW` (e.g. a pending notification dispatch) has run. The loop quits
/// when the test future completes.
pub fn run_in_glib_main_loop<F, U>(f: F)
where
	F: FnOnce(mpsc::Receiver<()>) -> U + Send + 'static,
	U: Future<Output = ()>,
{
	let main_loop = Arc::new(MainLoop::new(None, false));
	let main_context = main_loop.context();

	// The idea is to have an asynchronous test case that can yield control back to
	// the main loop. We do that with a separate low-priority task that, every time
	// it is polled, wakes up the test code. In effect, every time the test code
	// yields, it is resumed when no tasks higher than PRIORITY_LOW are ready.
	let (mut yield_tx, yield_rx) = mpsc::channel(0);

	main_context.spawn_with_priority(glib::Priority::LOW, async move {
		loop {
			yield_tx.send(()).await.unwrap();
		}
	});

	let main_loop_clone = main_loop.clone();
	main_context.spawn(async move {
		let main_context = main_loop_clone.context();
		main_context.spawn_local(async move {
			// Quit main loop after test code completes.
			f(yield_rx).await;
			main_loop_clone.quit();
		});
	});

	main_loop.run();
}

/// The DSP chain `config` describes, reading its audio from `reader`.
///
/// The same wiring `AppController` performs when it starts the spectrum thread,
/// with no thread and no JACK client: the generator `config` names, then its
/// transforms in configured order.
pub fn renderer(config: &Config, reader: SampleReader, sample_rate: u32) -> SpectrumRenderer {
	let SpectrumGeneratorConfig::Audio(generator_config) = &config.spectrum_generator;

	let mut renderer = SpectrumRenderer::new();
	renderer.set_generator(Box::new(AudioSpectrumGenerator::new(
		generator_config.clone(),
		reader,
		sample_rate,
	)));
	*renderer.transforms_mut() = config
		.spectrum_transforms
		.iter()
		.map(|(id, transform_config)| (*id, transform_config.clone().create()))
		.collect();
	renderer
}

/// The graphic stage `config` describes, holding one frame of `values`.
///
/// The same wiring `AppController` performs when it starts the graphic thread,
/// with no thread and no GTK: the generator `config` names, on the frequency
/// grid `config` implies. The renderer learns its surface size from the first
/// buffer [`render`](GraphicRenderer::render) is handed.
///
/// The spectrum is built on the renderer's own grid, so it lands in the history
/// rather than taking the stale-grid path.
///
/// # Preconditions
/// - `values.len()` equals `config.spectrum_params().samples()`
pub fn graphic_renderer(config: &Config, values: &[f64]) -> GraphicRenderer {
	let mut renderer = GraphicRenderer::new();
	renderer.set_spectrum_params(config.spectrum_params());
	renderer.update_generator(|generator| config.graphic_generator.clone().update(generator));

	let buffer = SpectrumBuffer::new(renderer.spectrum_params().clone());
	assert_eq!(values.len(), buffer.params().samples());
	renderer.update_spectrum(buffer.fill(|data, _params| data.copy_from_slice(values)));
	renderer
}

/// Seconds one call to `work` takes, averaged over a fixed run.
///
/// The measurement the budget tests and the benchmark summaries make: enough
/// repetitions to average out scheduling noise, no criterion machinery.
pub fn seconds_per_call(warmup: usize, runs: usize, mut work: impl FnMut()) -> f64 {
	for _ in 0..warmup {
		work();
	}
	let start = Instant::now();
	for _ in 0..runs {
		work();
	}
	start.elapsed().as_secs_f64() / runs as f64
}
