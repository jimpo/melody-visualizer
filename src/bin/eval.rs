//! Offline eval runner: a WAV file through the spectrum generator and the
//! transform chain at a fixed tick.
//!
//! No threads, no plugin, no window. One spectrum per tick, written as CSV rows
//! of the bins, plus the time each tick took. The chain is read from a TOML
//! file holding the plugin's own `Config`, so changing parameters, order or
//! membership needs no rebuild. The run is deterministic: the same input and
//! config give the same spectra, byte for byte.
//!
//! ```text
//! eval [--config chain.toml] [--out spectra.csv] input.wav
//! eval --dump-config > chain.toml
//! ```

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use melody_visualizer::app::Config;
use melody_visualizer::audio::ring::sample_ring;
use melody_visualizer::spectrum::generators::audio::{self, AudioSpectrumGenerator, MAX_WINDOW_MS};
use melody_visualizer::spectrum::{SpectrumBuffer, SpectrumGenerator, TransformChain};

const USAGE: &str =
	"usage: eval [--config chain.toml] [--out spectra.csv] input.wav\n       eval --dump-config";

fn main() -> ExitCode {
	match run() {
		Ok(()) => ExitCode::SUCCESS,
		Err(message) => {
			eprintln!("eval: {message}");
			ExitCode::FAILURE
		}
	}
}

fn run() -> Result<(), String> {
	let mut config_path = None;
	let mut out_path = None;
	let mut input = None;
	let mut args = std::env::args().skip(1);
	while let Some(arg) = args.next() {
		match arg.as_str() {
			"--config" => config_path = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
			"--out" => out_path = Some(PathBuf::from(args.next().ok_or(USAGE)?)),
			"--dump-config" => {
				let text = toml::to_string_pretty(&Config::default()).map_err(|e| e.to_string())?;
				print!("{text}");
				return Ok(());
			}
			_ if arg.starts_with("--") => return Err(format!("unknown option {arg}\n{USAGE}")),
			_ if input.is_none() => input = Some(PathBuf::from(arg)),
			_ => return Err(USAGE.to_string()),
		}
	}
	let input = input.ok_or(USAGE)?;

	let config: Config = match config_path {
		Some(path) => {
			let text = std::fs::read_to_string(&path)
				.map_err(|e| format!("reading {}: {e}", path.display()))?;
			toml::from_str(&text).map_err(|e| format!("parsing {}: {e}", path.display()))?
		}
		None => Config::default(),
	};

	let (samples, sample_rate) = read_wav(&input)?;
	let audio_config = config.spectrum_generator.audio.clone();
	let update_rate = audio_config.update_rate.max(1);

	// The ring is the same one the plugin reads through. It holds the longest
	// window plus a hop, so nothing is dropped between ticks.
	let hop_max = sample_rate.div_ceil(update_rate) as usize;
	let capacity = audio::window_samples(MAX_WINDOW_MS, sample_rate) + hop_max;
	let (mut writer, reader) = sample_ring(capacity);
	let mut generator = AudioSpectrumGenerator::new(audio_config, reader, sample_rate);

	let params = Arc::new(config.spectrum_params());
	let mut chain: TransformChain = config.spectrum_transforms.iter().cloned().collect();
	chain.set_params(&params);

	let mut out = match &out_path {
		Some(path) => Some(BufWriter::new(
			File::create(path).map_err(|e| format!("creating {}: {e}", path.display()))?,
		)),
		None => None,
	};

	let mut buffer = SpectrumBuffer::new(params.clone());
	let mut times = Vec::new();
	let mut consumed = 0usize;
	let mut tick = 0u64;
	loop {
		// The end of this tick's hop, in whole samples. Computed from the tick
		// number so rounding never accumulates.
		tick += 1;
		let end = ((tick as u128 * sample_rate as u128) / update_rate as u128) as usize;
		if end > samples.len() {
			break;
		}
		writer.write_samples(&samples[consumed..end]);
		consumed = end;

		let start = Instant::now();
		let spectrum = chain.apply(generator.generate(buffer));
		times.push(start.elapsed().as_secs_f64());

		if let Some(out) = out.as_mut() {
			let row: Vec<String> = spectrum.values().iter().map(|v| format!("{v:e}")).collect();
			writeln!(out, "{}", row.join(",")).map_err(|e| e.to_string())?;
		}
		buffer = spectrum.into_buffer();
	}
	if let Some(mut out) = out {
		out.flush().map_err(|e| e.to_string())?;
	}

	report(&times, sample_rate, update_rate);
	Ok(())
}

/// Reads `path` as mono `f32` in [-1, 1], averaging channels.
fn read_wav(path: &PathBuf) -> Result<(Vec<f32>, u32), String> {
	let mut reader =
		hound::WavReader::open(path).map_err(|e| format!("opening {}: {e}", path.display()))?;
	let spec = reader.spec();
	let channels = spec.channels as usize;
	let interleaved: Vec<f32> = match spec.sample_format {
		hound::SampleFormat::Float => reader
			.samples::<f32>()
			.collect::<Result<_, _>>()
			.map_err(|e| e.to_string())?,
		hound::SampleFormat::Int => {
			let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
			reader
				.samples::<i32>()
				.map(|s| s.map(|s| s as f32 * scale))
				.collect::<Result<_, _>>()
				.map_err(|e| e.to_string())?
		}
	};
	let mono = interleaved
		.chunks_exact(channels)
		.map(|frame| frame.iter().sum::<f32>() / channels as f32)
		.collect();
	Ok((mono, spec.sample_rate))
}

fn report(times: &[f64], sample_rate: u32, update_rate: u32) {
	if times.is_empty() {
		eprintln!("no ticks: the input is shorter than one hop");
		return;
	}
	let mut sorted = times.to_vec();
	sorted.sort_by(f64::total_cmp);
	let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
	eprintln!(
		"{} ticks at {update_rate}/s ({sample_rate} Hz): median {:.1} us, p95 {:.1} us, max {:.1} us (budget {:.1} us)",
		times.len(),
		at(0.5) * 1e6,
		at(0.95) * 1e6,
		sorted[sorted.len() - 1] * 1e6,
		1e6 / update_rate as f64,
	);
}
