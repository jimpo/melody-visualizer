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
/// At the longest window and the fastest rate it takes about 14% of the 10 ms
/// tick in a release build and 17% in the dev profile, which `Cargo.toml`
/// optimises, so one gate serves both. Three times' margin keeps a shared or
/// loaded machine from failing the build over noise.
const BUDGET: f64 = 0.5;

/// The windows the **Window** slider reaches, in milliseconds: its shortest,
/// the default, and its longest.
///
/// The window sets what one render costs and the update rate sets the tick, so
/// each runs at the fastest rate the slider offers, and the longest window,
/// with the most samples to transform and bin, has the least headroom.
const WINDOWS: [u32; 3] = [audio::MIN_WINDOW_MS, 50, audio::MAX_WINDOW_MS];

#[test]
fn the_chain_stays_well_inside_its_tick_at_every_setting_the_sliders_reach() {
	for window_ms in WINDOWS {
		let mut config = Config::default();
		let SpectrumGeneratorConfig::Audio(generator_config) = &mut config.spectrum_generator;
		generator_config.window_ms = window_ms;
		generator_config.update_rate = audio::MAX_UPDATE_RATE;
		check_one_window(&config);
	}
}

fn check_one_window(config: &Config) {
	let SpectrumGeneratorConfig::Audio(generator_config) = &config.spectrum_generator;
	let signal = sine_wave(
		&[(440.0, 1.0), (1760.0, 0.5), (7040.0, 0.25)],
		SAMPLE_RATE,
		audio::window_samples(generator_config.window_ms, SAMPLE_RATE),
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
		"at a {} ms window the chain took {:.3} ms of a {:.3} ms tick \
		 ({:.1}%, budget {:.0}%)",
		generator_config.window_ms,
		per_render.as_secs_f64() * 1000.0,
		tick.as_secs_f64() * 1000.0,
		share * 100.0,
		BUDGET * 100.0,
	);
}
