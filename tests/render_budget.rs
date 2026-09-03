//! A gate on the visualizer's share of one frame.
//!
//! `benches/graphic.rs` is where the surface and bin-count sweeps live;
//! benchmarks nobody runs catch no regressions, so this is the one timing
//! assertion the ordinary test run makes for the graphic stage.
//!
//! It is looser than it looks, and deliberately so. The visualizer has about
//! six times the headroom the DSP has fifty times of, so the threshold cannot
//! be set an order of magnitude clear of the measurement the way
//! `tests/dsp_budget.rs` sets its own. It is placed to fail on a genuine
//! regression and not on a loaded machine.

use melody_visualizer::app::Config;
use melody_visualizer::graphic::GraphicBuffer;
use melody_visualizer::test_support::{graphic_renderer, seconds_per_call};

/// The frame timer `VisualizationController` runs on: 25 fps.
const FRAME_SECONDS: f64 = 0.040;
/// A window-sized surface, the middle of the range the benchmark sweeps.
const SURFACE: (i32, i32) = (1280, 800);
const WARMUP: usize = 10;
const FRAMES: usize = 50;

/// The share of one frame the render may take.
///
/// One threshold serves both profiles, unlike `tests/dsp_budget.rs`. Almost all
/// of this stage's time is inside cairo, which is compiled optimized either
/// way, so an unoptimized build measures within a fifth of a release one — the
/// mesh the Rust code assembles is not what the frame costs.
///
/// A frame takes 10-13% of the budget, so this leaves about three times the
/// margin. That is as much as a stage with six times' headroom can be given
/// while the gate still means anything.
const BUDGET: f64 = 0.40;

#[test]
fn a_frame_stays_inside_the_frame_timer() {
	let config = Config::default();
	// Every bin carrying energy is the most expensive mesh to paint.
	let values = vec![1.0; config.spectrum_params().samples()];

	let mut renderer = graphic_renderer(&config, &values);
	let (width, height) = SURFACE;
	let mut buffer = GraphicBuffer::new(width, height);

	// The steady state the graphic thread runs in: one buffer, recycled.
	let seconds = seconds_per_call(WARMUP, FRAMES, || {
		buffer = renderer
			.render(std::mem::take(&mut buffer))
			.expect("rasterising onto an in-memory surface needs no display")
			.into_buffer();
	});

	let share = seconds / FRAME_SECONDS;
	assert!(
		share < BUDGET,
		"a {}x{} frame took {:.3} ms of a {:.3} ms budget ({:.1}%, budget {:.0}%)",
		width,
		height,
		seconds * 1000.0,
		FRAME_SECONDS * 1000.0,
		share * 100.0,
		BUDGET * 100.0,
	);
}
