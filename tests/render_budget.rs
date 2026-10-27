//! A gate on the visualizer's share of one frame.
//!
//! `benches/graphic.rs` is where the surface and bin-count sweeps live;
//! benchmarks nobody runs catch no regressions, so this is the one timing
//! assertion the ordinary test run makes for the graphic stage. It is placed to
//! fail on a genuine regression and not on a loaded machine.

use melody_visualizer::app::Config;
use melody_visualizer::graphic::GraphicBuffer;
use melody_visualizer::test_support::{graphic_renderer, seconds_per_call};

/// The editor's fastest frame rate: nice-plug-iced polls for a frame 64 times
/// a second.
const FRAME_SECONDS: f64 = 1.0 / 64.0;
/// A window-sized surface, the middle of the range the benchmark sweeps.
const SURFACE: (i32, i32) = (1280, 800);
const WARMUP: usize = 10;
const FRAMES: usize = 50;

/// The share of one frame the render may take.
///
/// One threshold serves both profiles, unlike `tests/dsp_budget.rs`, because
/// `Cargo.toml` optimises the dev profile the tests build in. A frame takes
/// about 3% of the budget in a release build and 6% in a test one, so this
/// leaves about four times the margin.
const BUDGET: f64 = 0.25;

#[test]
fn a_frame_stays_inside_the_frame_timer() {
	let config = Config::default();
	// A loud frame, every bin at full scale.
	let values = vec![1.0; config.spectrum_params().samples()];

	let mut renderer = graphic_renderer(&config, &values);
	let (width, height) = SURFACE;
	let mut buffer = GraphicBuffer::new(width, height);

	// The steady state the graphic thread runs in: one buffer, recycled.
	let seconds = seconds_per_call(WARMUP, FRAMES, || {
		buffer = renderer
			.render(std::mem::take(&mut buffer))
			.expect("rendering into an in-memory buffer needs no display")
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
