//! Render throughput for the graphic stage.
//!
//! The stage converts spectra into frames, so it reports **frames/s**, measured
//! saturated — the history is already populated, so what is measured is the
//! draw rather than the DSP feeding it. The rate required is 25 fps, the 40 ms
//! `glib::timeout_add_local` in `VisualizationController`.
//!
//! This is the stage with the least headroom in the pipeline. The whole DSP
//! chain costs a tenth of a percent of its own tick; one frame at 1600x1000
//! costs about a sixth of this one.
//!
//! Two sweeps, and they scale opposite to the DSP:
//!
//! 1. **Surface area** dominates. cairo rasterises the mesh gradient over every
//!    pixel, and the cost tracks the pixel count.
//! 2. **Bin count** is nearly flat. Sixteen times the bins costs well under
//!    twice the time, because the patches are not what the time goes on.
//!
//! So adding spectral resolution is close to free for the visualizer while
//! resizing the window is what costs — the opposite of the DSP, where bins drive
//! a quadratic. Anyone tuning `samples_per_octave` needs both facts together.
//!
//! The `background` group fills the surface and draws no mesh. `render` minus
//! `background` is what the mesh paint costs, which is the number to look at
//! before clipping the paint to the annulus.
//!
//! Run with `cargo bench --bench graphic`. The summary alone is
//! `cargo bench --bench graphic -- --test`.

use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};
use rand::{RngExt, SeedableRng, rngs::StdRng};

use melody_visualizer::app::Config;
use melody_visualizer::graphic::renderer::GraphicRenderer;
use melody_visualizer::graphic::{Graphic, GraphicBuffer};
use melody_visualizer::test_support::{graphic_renderer, seconds_per_call};

/// The frame timer `VisualizationController` runs on: 25 fps.
const FRAME_BUDGET: Duration = Duration::from_millis(40);
/// The surface the regression gate in `tests/render_budget.rs` watches.
const GATE_SURFACE: (i32, i32) = (1280, 800);

/// Surfaces from a small window to 4K, by pixel count.
const SURFACES: [(i32, i32); 6] = [
	(640, 480),
	(800, 600),
	(1280, 800),
	(1600, 1000),
	(1920, 1080),
	(3840, 2160),
];

/// A spectrum's worth of broadband values, seeded so every run draws the same
/// frame.
///
/// Every bin carries energy, which is what the mesh gradient costs most to
/// paint and what real music gives the visualizer. A chord would leave most of
/// the spiral at the base brightness, and cairo's work would not change much —
/// but the values would no longer be the ones the app draws.
fn values(samples: usize) -> Vec<f64> {
	let mut rng = StdRng::seed_from_u64(0);
	(0..samples).map(|_| rng.random_range(0.0..1.0)).collect()
}

/// The configured stage at `samples_per_octave`, and the bin count it produces.
fn stage(samples_per_octave: usize) -> (GraphicRenderer, usize) {
	let config = Config {
		samples_per_octave,
		..Config::default()
	};
	let bins = config.spectrum_params().samples();
	(graphic_renderer(&config, &values(bins)), bins)
}

/// Seconds one frame at `width` by `height` takes out of `renderer`.
fn seconds_per_frame(renderer: &mut GraphicRenderer, width: i32, height: i32) -> f64 {
	let mut buffer = GraphicBuffer::new(width, height);
	seconds_per_call(20, 100, || {
		buffer = render(renderer, std::mem::take(&mut buffer)).into_buffer();
	})
}

/// One frame, recycling `buffer` the way the GTK thread does.
fn render(renderer: &mut GraphicRenderer, buffer: GraphicBuffer) -> Graphic {
	renderer
		.render(buffer)
		.expect("rasterising onto an in-memory surface needs no display")
}

/// The black fill `generate` lays down before painting the mesh over it.
fn background(buffer: GraphicBuffer) -> Graphic {
	let (width, height) = (buffer.width() as f64, buffer.height() as f64);
	buffer
		.draw(|ctx| {
			ctx.set_source_rgb(0.0, 0.0, 0.0);
			ctx.rectangle(0.0, 0.0, width, height);
			ctx.fill()?;
			Ok(())
		})
		.expect("a plain fill needs no display")
}

fn bench_surface(criterion: &mut Criterion) {
	let mut group = criterion.benchmark_group("render");
	// The stage produces one frame per call, whatever the surface.
	group.throughput(Throughput::Elements(1));
	let (mut renderer, _bins) = stage(Config::default().samples_per_octave);

	for (width, height) in SURFACES {
		let size = format!("{}x{}", width, height);
		group.bench_function(BenchmarkId::new("spiral", &size), |bencher| {
			let mut buffer = GraphicBuffer::new(width, height);
			bencher.iter(|| {
				buffer = render(&mut renderer, std::mem::take(&mut buffer)).into_buffer();
			});
		});
		group.bench_function(BenchmarkId::new("background", &size), |bencher| {
			let mut buffer = GraphicBuffer::new(width, height);
			bencher.iter(|| {
				buffer = background(std::mem::take(&mut buffer)).into_buffer();
			});
		});
	}
	group.finish();
}

fn bench_bins(criterion: &mut Criterion) {
	let mut group = criterion.benchmark_group("render_bins");
	group.throughput(Throughput::Elements(1));
	let (width, height) = (1600, 1000);

	for samples_per_octave in [45, Config::default().samples_per_octave, 720] {
		let (mut renderer, bins) = stage(samples_per_octave);
		group.bench_function(BenchmarkId::new("spiral", bins), |bencher| {
			let mut buffer = GraphicBuffer::new(width, height);
			bencher.iter(|| {
				buffer = render(&mut renderer, std::mem::take(&mut buffer)).into_buffer();
			});
		});
	}
	group.finish();
}

/// Print each surface's frame rate and its share of the frame budget.
fn print_summary() {
	let (mut renderer, bins) = stage(Config::default().samples_per_octave);
	let required = 1.0 / FRAME_BUDGET.as_secs_f64();

	println!();
	println!(
		"Graphic throughput: {} bins, {:.3} ms frame ({:.1} frames/s required)",
		bins,
		FRAME_BUDGET.as_secs_f64() * 1000.0,
		required,
	);
	println!(
		"{:<20} {:>14} {:>12} {:>12}",
		"surface", "capacity /s", "headroom", "% of budget"
	);
	for (width, height) in SURFACES {
		let seconds = seconds_per_frame(&mut renderer, width, height);
		print_row(&format!("{}x{}", width, height), seconds, required);
	}

	println!();
	println!(
		"Bin count at {}x{} (the surface is what costs, not the bins):",
		1600, 1000
	);
	for samples_per_octave in [45, Config::default().samples_per_octave, 720] {
		let (mut renderer, bins) = stage(samples_per_octave);
		let seconds = seconds_per_frame(&mut renderer, 1600, 1000);
		print_row(&format!("{} bins", bins), seconds, required);
	}

	println!(
		"\n(the gate in tests/render_budget.rs watches {}x{})",
		GATE_SURFACE.0, GATE_SURFACE.1,
	);
}

fn print_row(name: &str, seconds: f64, required: f64) {
	let capacity = 1.0 / seconds;
	println!(
		"{:<20} {:>14.0} {:>11.0}x {:>11.2}%",
		name,
		capacity,
		capacity / required,
		100.0 * required * seconds,
	);
}

fn main() {
	let mut criterion = Criterion::default()
		.warm_up_time(Duration::from_millis(500))
		.measurement_time(Duration::from_secs(2))
		.configure_from_args();
	bench_surface(&mut criterion);
	bench_bins(&mut criterion);
	criterion.final_summary();

	print_summary();
}
