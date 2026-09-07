//! A gate on the DSP's share of its own tick.
//!
//! `benches/dsp.rs` is where the per-stage numbers and the diffuser's cost curve
//! live; benchmarks nobody runs catch no regressions, so this is the one timing
//! assertion the ordinary test run makes. It is deliberately loose: it fails on
//! a catastrophic regression, not on a loaded machine.

use std::sync::Arc;
use std::time::Instant;

use melody_visualizer::app::Config;
use melody_visualizer::app::config::SpectrumGeneratorConfig;
use melody_visualizer::spectrum::SpectrumBuffer;
use melody_visualizer::spectrum::generators::audio;
use melody_visualizer::test_support::{renderer, sample_reader, sine_wave};

const SAMPLE_RATE: u32 = 48_000;
const RENDERS: usize = 200;

/// The share of one tick the chain may take.
///
/// It takes 0.13% of it in a release build and 3.4% unoptimized, and
/// `cargo nextest run` measures the unoptimized one by default — so the gate is
/// set per profile. Both leave more than an order of magnitude of margin, which
/// is what keeps a shared or loaded machine from failing the build over noise.
const BUDGET: f64 = if cfg!(debug_assertions) { 0.5 } else { 0.1 };

/// The grids the **Pitch resolution** slider reaches, in samples per octave:
/// its two ends and the default between them.
///
/// The two ends pull in opposite directions. The top has twice the bins, which
/// costs the diffuser four times as much, but its window is twice as long and
/// so is its tick. The bottom is the short-window end, where a 128-sample
/// window ticks every 1.3 ms and the share is what a cheap chain keeps of a
/// tick that short.
const GRIDS: [usize; 3] = [12, 180, 360];

#[test]
fn the_chain_stays_well_inside_its_tick_at_every_grid_the_slider_reaches() {
	for samples_per_octave in GRIDS {
		let mut config = Config {
			samples_per_octave,
			..Default::default()
		};
		let SpectrumGeneratorConfig::Audio(generator_config) = &mut config.spectrum_generator;
		generator_config.dft_window_size = audio::dft_window_size(samples_per_octave);
		check_one_grid(&config);
	}
}

fn check_one_grid(config: &Config) {
	let SpectrumGeneratorConfig::Audio(generator_config) = &config.spectrum_generator;
	let signal = sine_wave(
		&[(440.0, 1.0), (1760.0, 0.5), (7040.0, 0.25)],
		SAMPLE_RATE,
		generator_config.dft_window_size,
	);

	let mut renderer = renderer(config, sample_reader(&signal), SAMPLE_RATE);
	let tick = renderer.generator().interval();
	let mut buffer = SpectrumBuffer::new(Arc::new(config.spectrum_params()));

	// The steady state the spectrum thread runs in: one buffer, recycled, with
	// the generator's input never running dry.
	for _ in 0..RENDERS / 10 {
		buffer = renderer.render(buffer).into_buffer();
	}
	let start = Instant::now();
	for _ in 0..RENDERS {
		buffer = renderer.render(buffer).into_buffer();
	}
	let per_render = start.elapsed() / RENDERS as u32;

	let share = per_render.as_secs_f64() / tick.as_secs_f64();
	assert!(
		share < BUDGET,
		"at {} bins / octave the chain took {:.3} ms of a {:.3} ms tick \
		 ({:.1}%, budget {:.0}%)",
		config.samples_per_octave,
		per_render.as_secs_f64() * 1000.0,
		tick.as_secs_f64() * 1000.0,
		share * 100.0,
		BUDGET * 100.0,
	);
}
