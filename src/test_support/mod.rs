//! Shared test-only helpers.
//!
//! Compiled for the crate's own tests, and for dependents that enable the
//! `testing` feature.

use std::{f64::consts::TAU, sync::Arc, time::Instant};

use crate::app::config::Config;
use crate::audio::SampleReader;
use crate::audio::ring::sample_ring;
use crate::graphic::renderer::GraphicRenderer;
use crate::spectrum::generators::cqt::CqtSpectrumGenerator;
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

/// A [`SampleReader`] holding `samples`, with no host behind it.
///
/// This is the seam the generator is fed deterministic audio through. The reader peeks rather
/// than consumes, so the same samples serve every call to
/// [`generate`](crate::spectrum::SpectrumGenerator::generate).
pub fn sample_reader(samples: &[f32]) -> SampleReader {
	let (mut writer, reader) = sample_ring(samples.len());
	writer.write_samples(samples);
	assert_eq!(
		reader.overruns(),
		0,
		"the ring was sized to hold every sample",
	);
	reader
}

/// The DSP chain `config` describes, reading its audio from `reader`.
///
/// The wiring the spectrum thread is started with, with no thread and no host,
/// on the audio path: the audio generator `config` describes, then its
/// transforms in configured order. The engine's `SourceGenerator` draws the
/// same until a note is played.
pub fn renderer(config: &Config, reader: SampleReader, sample_rate: u32) -> SpectrumRenderer {
	let generator_config = &config.spectrum_generator.audio;

	let mut renderer = SpectrumRenderer::new();
	renderer.set_generator(Box::new(CqtSpectrumGenerator::new(
		generator_config.clone(),
		reader,
		sample_rate,
	)));
	*renderer.transforms_mut() = config.spectrum_transforms.iter().cloned().collect();
	renderer
}

/// The graphic stage `config` describes, holding one frame of `values`.
///
/// The wiring the graphic thread is started with, with no thread and no
/// window: the generator `config` names, on the frequency grid `config`
/// implies. The renderer learns its surface size from the first
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
