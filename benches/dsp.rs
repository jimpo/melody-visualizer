//! Per-stage throughput for the DSP chain.
//!
//! Every stage converts one thing into another, so each is measured in its own
//! unit and saturated — fed as fast as it can consume, so the number is the
//! stage's own capacity rather than the rate the pipeline happens to run at.
//!
//! | Stage | Input → output | Reported as |
//! | -- | -- | -- |
//! | DFT | samples → complex bins | samples/s |
//! | Log binning | `n/2` complex bins → log-spaced bins | spectra/s |
//! | Generator (both) | samples → spectra | spectra/s |
//! | Each transform | spectra → spectra | spectra/s |
//!
//! Every stage runs in sequence on the one spectrum thread, so the chain's
//! capacity is the reciprocal sum of the stages', not the smallest of them. The
//! two agree while one stage dominates, as one does today, and diverge as soon
//! as two are comparable — which is the regime the diffuser's cost creates. The
//! summary at the end computes the composed figure so a reader does not take
//! the minimum and get it wrong.
//!
//! Run with `cargo bench`. The summary alone is `cargo bench -- --test`.

use std::sync::Arc;
use std::time::Duration;

use criterion::{BenchmarkId, Criterion, Throughput};
use rand::{RngExt, SeedableRng, rngs::StdRng};

use melody_visualizer::app::Config;
use melody_visualizer::app::config::SpectrumGeneratorConfig;
use melody_visualizer::spectrum::generators::audio::{
	self, Analyzer, AudioSpectrumGenerator, WindowShape,
};
use melody_visualizer::spectrum::renderer::SpectrumRenderer;
use melody_visualizer::spectrum::transforms::{
	DecibelConverter, Diffuser, VolumeNormalizer, decibel_converter, diffuser, volume_normalizer,
};
use melody_visualizer::spectrum::{
	Spectrum, SpectrumBuffer, SpectrumGenerator, SpectrumParams, SpectrumTransform,
};
use melody_visualizer::test_support::{renderer, sample_reader, seconds_per_call};
use melody_visualizer::traits::Configurable;

const SAMPLE_RATE: u32 = 48_000;
/// The overlap the default configuration analyses at.
const OVERLAP: f64 = 0.5;
/// Repetitions the summary averages one stage over.
const WARMUP: usize = 50;
const RUNS: usize = 500;
/// The bin count the default configuration produces.
const DEFAULT_BINS: usize = 1196;
/// Broadband noise, seeded so every run measures the same work.
///
/// Every bin carries energy, which is what real music gives the DSP and what a
/// capacity figure should assume. A chord would leave most of the spectrum near
/// zero, and the decibel converter skips a bin below its floor — so the number
/// would flatter it by the fraction of the spectrum the notes happen to miss.
fn signal(samples: usize) -> Vec<f32> {
	let mut rng = StdRng::seed_from_u64(0);
	(0..samples)
		.map(|_| rng.random_range(-1.0..1.0f32))
		.collect()
}

fn grid(bins: usize) -> Arc<SpectrumParams> {
	Arc::new(SpectrumParams::exp_spaced(bins, 200.0, 20000.0))
}

/// A generator over a window's worth of the test signal.
///
/// The reader peeks rather than consumes, so it never runs dry: the stage
/// downstream of it is what the measurement is limited by.
fn generator(window: usize) -> AudioSpectrumGenerator {
	AudioSpectrumGenerator::new(
		audio::Config {
			dft_window_size: window,
			overlap: OVERLAP,
		},
		sample_reader(&signal(window)),
		SAMPLE_RATE,
	)
}

/// An analyzer with the test signal already transformed, ready to bin.
fn analyzer(window: usize) -> Analyzer {
	let mut analyzer = Analyzer::new(WindowShape::Hann, SAMPLE_RATE);
	analyzer.set_window_size(window);
	analyzer.run_dft(signal(window).into_iter().map(f64::from));
	analyzer
}

/// A spectrum of `bins` bins as a transform in the chain would receive it:
/// analyzed, smoothed and normalized.
///
/// The values matter as well as their number. The decibel converter skips a bin
/// below its floor, so a raw generator spectrum — most of whose bins are far
/// below it — would measure the branch rather than the logarithm.
fn spectrum(bins: usize) -> Spectrum {
	let params = grid(bins);
	let mut renderer = SpectrumRenderer::new();
	renderer.set_generator(Box::new(generator(2048)));
	*renderer.transforms_mut() = Config::default().spectrum_transforms.into_iter().collect();
	renderer.render(SpectrumBuffer::new(params))
}

/// Run `spectrum` through `transform` in place, recycling the buffer.
///
/// Nothing is allocated per call, and the transform is fed as fast as it can
/// consume — so what is measured is the stage rather than its input.
fn apply(transform: &mut dyn SpectrumTransform, spectrum: &mut Spectrum) {
	*spectrum = transform.transform(std::mem::take(spectrum));
}

/// The transforms, each named and built at the given bin count.
fn transforms(bins: usize) -> Vec<(String, Box<dyn SpectrumTransform>)> {
	let params = grid(bins);
	let mut built: Vec<(String, Box<dyn SpectrumTransform>)> = vec![
		(
			"diffuser".to_string(),
			Box::new(Diffuser::new(diffuser::Config { width: 1.0 / 24.0 })),
		),
		(
			"volume_normalizer".to_string(),
			Box::new(VolumeNormalizer::new(volume_normalizer::Config {
				rate: 0.1,
			})),
		),
		(
			"decibel_converter".to_string(),
			Box::new(DecibelConverter::new(decibel_converter::Config {
				min_level: 1.0e-6,
			})),
		),
	];
	for (_name, transform) in built.iter_mut() {
		transform.set_params(&params);
	}
	built
}

fn bench_generator(criterion: &mut Criterion) {
	let mut group = criterion.benchmark_group("generator");
	for window in [512, 1024, 2048, 4096, 8192] {
		let grid = grid(DEFAULT_BINS);

		// The DFT converts samples, so it is measured in them.
		group.throughput(Throughput::Elements(window as u64));
		group.bench_function(BenchmarkId::new("dft", window), |bencher| {
			let mut analyzer = analyzer(window);
			let samples = signal(window);
			bencher.iter(|| analyzer.run_dft(samples.iter().map(|&sample| sample as f64)));
		});

		// Binning and the generator as a whole each produce one spectrum.
		group.throughput(Throughput::Elements(1));
		group.bench_function(BenchmarkId::new("binning", window), |bencher| {
			let analyzer = analyzer(window);
			let mut buffer = SpectrumBuffer::new(grid.clone());
			bencher.iter(|| {
				buffer = analyzer
					.fill_bins(std::mem::take(&mut buffer))
					.into_buffer();
			});
		});
		group.bench_function(BenchmarkId::new("spectrum", window), |bencher| {
			let mut generator = generator(window);
			let mut buffer = SpectrumBuffer::new(grid.clone());
			bencher.iter(|| {
				buffer = generator
					.generate(std::mem::take(&mut buffer))
					.into_buffer();
			});
		});
	}
	group.finish();
}

fn bench_transforms(criterion: &mut Criterion) {
	let mut group = criterion.benchmark_group("transform");
	group.throughput(Throughput::Elements(1));
	for bins in [DEFAULT_BINS, 2 * DEFAULT_BINS, 4 * DEFAULT_BINS] {
		let input = spectrum(bins);
		for (name, mut transform) in transforms(bins) {
			group.bench_function(BenchmarkId::new(name, bins), |bencher| {
				let mut spectrum = input.clone();
				bencher.iter(|| apply(transform.as_mut(), &mut spectrum));
			});
		}
	}
	group.finish();
}

fn bench_diffuser_width(criterion: &mut Criterion) {
	let mut group = criterion.benchmark_group("diffuser_width");
	group.throughput(Throughput::Elements(1));
	let input = spectrum(DEFAULT_BINS);
	// The only superlinear parameter: the window grows with the width, and
	// convolution costs bins × window.
	for (name, width) in [
		("1_24_octave", 1.0 / 24.0),
		("1_octave", 1.0),
		("10_octaves", 10.0),
	] {
		let mut diffuser = Diffuser::new(diffuser::Config { width });
		diffuser.set_params(&grid(DEFAULT_BINS));
		group.bench_function(name, |bencher| {
			let mut spectrum = input.clone();
			bencher.iter(|| apply(&mut diffuser, &mut spectrum));
		});
	}
	group.finish();
}

fn bench_chain(criterion: &mut Criterion) {
	let mut group = criterion.benchmark_group("chain");
	group.throughput(Throughput::Elements(1));
	group.bench_function("default", |bencher| {
		let (mut renderer, mut buffer) = default_chain();
		bencher.iter(|| {
			buffer = renderer.render(std::mem::take(&mut buffer)).into_buffer();
		});
	});
	group.finish();
}

/// The chain the app runs, and a buffer to recycle through it.
fn default_chain() -> (SpectrumRenderer, SpectrumBuffer) {
	let config = Config::default();
	let SpectrumGeneratorConfig::Audio(generator_config) = &config.spectrum_generator;
	let signal = signal(generator_config.dft_window_size);
	let renderer = renderer(&config, sample_reader(&signal), SAMPLE_RATE);
	let buffer = SpectrumBuffer::new(Arc::new(config.spectrum_params()));
	(renderer, buffer)
}

/// Print each stage's capacity, the capacity of the whole chain, and how much of
/// the tick budget the chain uses.
///
/// The composed capacity is the reciprocal sum, because the stages share one
/// thread. Headroom is against the tick rate the configured window implies, not
/// a fixed constant: a transform's cost depends on the bin count, but the rate
/// demanded of it is `1 / interval`, which scales with the window.
fn print_summary() {
	let config = Config::default();
	let grid = grid(DEFAULT_BINS);

	let mut stages = Vec::new();

	let mut generator = {
		let SpectrumGeneratorConfig::Audio(generator_config) = &config.spectrum_generator;
		generator(generator_config.dft_window_size)
	};
	let tick = generator.interval();
	let mut buffer = SpectrumBuffer::new(grid.clone());
	stages.push((
		"generator",
		seconds_per_call(WARMUP, RUNS, || {
			buffer = generator
				.generate(std::mem::take(&mut buffer))
				.into_buffer();
		}),
	));

	for (name, mut transform) in transforms(DEFAULT_BINS) {
		let mut spectrum = spectrum(DEFAULT_BINS);
		let seconds = seconds_per_call(WARMUP, RUNS, || apply(transform.as_mut(), &mut spectrum));
		stages.push((
			match name.as_str() {
				"diffuser" => "diffuser",
				"volume_normalizer" => "volume normalizer",
				_ => "decibel converter",
			},
			seconds,
		));
	}

	let (mut renderer, mut chain_buffer) = default_chain();
	let composed = seconds_per_call(WARMUP, RUNS, || {
		chain_buffer = renderer
			.render(std::mem::take(&mut chain_buffer))
			.into_buffer();
	});

	let required = 1.0 / tick.as_secs_f64();
	println!();
	println!(
		"DSP throughput: {} bins, {} Hz, {:.3} ms per tick ({:.1} spectra/s required)",
		DEFAULT_BINS,
		SAMPLE_RATE,
		tick.as_secs_f64() * 1000.0,
		required,
	);
	println!(
		"{:<20} {:>14} {:>12} {:>12}",
		"stage", "capacity /s", "headroom", "% of tick"
	);
	for (name, seconds) in &stages {
		print_row(name, *seconds, required);
	}
	// Sequential stages on one thread: the reciprocals add.
	let reciprocal_sum = stages
		.iter()
		.filter(|(name, _)| *name != "decibel converter")
		.map(|(_, seconds)| seconds)
		.sum::<f64>();
	print_row("composed (sum)", reciprocal_sum, required);
	print_row("composed (measured)", composed, required);
	println!(
		"\n(the decibel converter is reported but left out of the composed chain: \
		 `Config::default` does not run it)",
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
	bench_generator(&mut criterion);
	bench_transforms(&mut criterion);
	bench_diffuser_width(&mut criterion);
	bench_chain(&mut criterion);
	criterion.final_summary();

	print_summary();
}
