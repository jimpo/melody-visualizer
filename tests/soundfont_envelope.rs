//! The eval SoundFonts' envelope against the MIDI generator's.
//!
//! `eval/soundfont/` holds two pianos whose volume envelope was written into
//! the SoundFont from `Envelope::default()`. This renders single notes across
//! the keyboard and at several velocities through FluidSynth, measures the
//! level of each note's fundamental over time, and compares it to the level
//! the MIDI generator gives the same note. It is a test, not a source of data:
//! if it fails, the reference spectrum does not describe the eval audio.
//!
//! It needs `fluidsynth` (on the PATH, or at `$FLUIDSYNTH`) and `python3`
//! for the render tool. Without them it skips, unless `$REQUIRE_FLUIDSYNTH` is
//! set, which CI should do.

use std::path::{Path, PathBuf};
use std::process::Command;

use melody_visualizer::reference::{self, KEYS};
use melody_visualizer::spectrum::generators::midi::Envelope;
use midly::num::{u4, u7, u15, u24, u28};
use midly::{Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

const SOUNDFONTS: [&str; 2] = ["generaluser", "fluidr3"];
const KEYS_TESTED: [u8; 11] = [36, 42, 48, 54, 60, 66, 72, 78, 84, 90, 96];
const VELOCITIES: [u8; 4] = [30, 64, 100, 127];

/// The notes of one render: each starts this long after the last one did.
const SPACING: f64 = 6.0;
const NOTE_ON: f64 = 0.5;
const NOTE_OFF: f64 = 3.5;
const SAMPLE_RATE: u32 = 44_100;
const TICK_RATE: u32 = 1000;
/// The measuring window. Long enough to resolve the lowest note's fundamental
/// from its first overtone, short against the envelope.
const WINDOW: f64 = 0.2;
const HOP: f64 = 0.1;

/// Points are compared while the envelope is within this many dB of the note's
/// peak. Below it a 16-bit render is quantisation noise.
const RANGE_DB: f64 = 30.0;
/// The most the measured level may stray from the generator's, once the note's
/// overall gain (the sample's loudness) is taken out.
const SHAPE_TOLERANCE_DB: f64 = 1.0;
/// The most the gain may differ between velocities of the same key: the
/// velocity curve.
const VELOCITY_TOLERANCE_DB: f64 = 0.5;

#[test]
fn rendered_notes_follow_the_midi_generators_envelope() {
	let Some(fluidsynth) = find_fluidsynth() else {
		assert!(
			std::env::var_os("REQUIRE_FLUIDSYNTH").is_none(),
			"REQUIRE_FLUIDSYNTH is set but fluidsynth was not found"
		);
		eprintln!("skipped: fluidsynth not found");
		return;
	};
	let envelope = Envelope::default();
	let dir = std::env::temp_dir().join(format!("soundfont-envelope-{}", std::process::id()));
	std::fs::create_dir_all(&dir).unwrap();

	let mut failures = Vec::new();
	for soundfont in SOUNDFONTS {
		for key in KEYS_TESTED {
			let notes: Vec<_> = VELOCITIES
				.iter()
				.enumerate()
				.map(|(i, &velocity)| {
					let start = i as f64 * SPACING;
					(key, velocity, start + NOTE_ON, start + NOTE_OFF)
				})
				.collect();
			let midi = dir.join("notes.mid");
			let bytes = midi_file(&notes);
			std::fs::write(&midi, &bytes).unwrap();
			let wav = dir.join("notes.wav");
			render(&fluidsynth, soundfont, &midi, &wav);
			let heard = read_mono(&wav);

			let end = notes.len() as f64 * SPACING;
			let events = reference::read_midi(&bytes).unwrap();
			let expected = reference::render(
				&events,
				&envelope,
				TICK_RATE,
				(end * f64::from(TICK_RATE)) as usize,
			);
			let column: Vec<f64> = expected
				.iter()
				.map(|row| row[usize::from(key) - 21])
				.collect();

			let frequency = 440.0 * ((f64::from(key) - 69.0) / 12.0).exp2();
			let mut gains = Vec::new();
			for (i, &velocity) in VELOCITIES.iter().enumerate() {
				let start = i as f64 * SPACING;
				let mut points = Vec::new();
				// Windows that include the onset are left out: the 5 ms attack
				// cannot be told from where the note starts at this length.
				let mut centre = NOTE_ON + WINDOW / 2.0;
				while centre < SPACING - WINDOW / 2.0 {
					let t = start + centre;
					let want = windowed(&column, t);
					let got = fundamental(&heard, frequency, t);
					points.push((want, got));
					centre += HOP;
				}
				let peak = points.iter().map(|p| p.0).fold(0.0, f64::max);
				let kept: Vec<(f64, f64)> = points
					.into_iter()
					.filter(|p| p.0 > peak * 10f64.powf(-RANGE_DB / 20.0))
					.map(|(want, got)| (20.0 * want.log10(), 20.0 * got.max(1e-12).log10()))
					.collect();
				let mut offsets: Vec<f64> = kept.iter().map(|(w, g)| g - w).collect();
				offsets.sort_by(f64::total_cmp);
				let gain = offsets[offsets.len() / 2];
				let worst = offsets.iter().map(|o| (o - gain).abs()).fold(0.0, f64::max);
				if worst > SHAPE_TOLERANCE_DB {
					failures.push(format!(
						"{soundfont} key {key} velocity {velocity}: envelope off by up to {worst:.2} dB"
					));
				}
				gains.push((velocity, gain));
			}
			let reference_gain = gains.last().unwrap().1;
			for (velocity, gain) in gains {
				if (gain - reference_gain).abs() > VELOCITY_TOLERANCE_DB {
					failures.push(format!(
						"{soundfont} key {key} velocity {velocity}: {:.2} dB from the curve",
						gain - reference_gain
					));
				}
			}
		}
	}
	let _ = std::fs::remove_dir_all(&dir);
	assert!(failures.is_empty(), "{}", failures.join("\n"));
}

fn find_fluidsynth() -> Option<PathBuf> {
	if let Some(path) = std::env::var_os("FLUIDSYNTH") {
		return Some(path.into());
	}
	let path = std::env::var_os("PATH")?;
	std::env::split_paths(&path)
		.map(|dir| dir.join("fluidsynth"))
		.find(|candidate| candidate.is_file())
}

/// Renders through the eval render tool, the one way eval audio is made.
fn render(fluidsynth: &Path, soundfont: &str, midi: &Path, wav: &Path) {
	let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("eval/soundfont");
	let status = Command::new("python3")
		.arg(root.join("render.py"))
		.arg("--soundfont")
		.arg(root.join(format!("{soundfont}.sf2")))
		.arg("--rate")
		.arg(SAMPLE_RATE.to_string())
		.arg(midi)
		.arg(wav)
		.env("FLUIDSYNTH", fluidsynth)
		.stderr(std::process::Stdio::null())
		.status()
		.expect("python3 runs the render tool");
	assert!(status.success(), "render.py failed for {soundfont}");
}

/// A Standard MIDI File of `(key, velocity, on, off)` notes in seconds, with
/// room after the last for its releases.
fn midi_file(notes: &[(u8, u8, f64, f64)]) -> Vec<u8> {
	const PPQ: f64 = 480.0;
	// 500 000 µs per quarter note: two quarters, 960 ticks, a second.
	let ticks = |seconds: f64| (seconds * 2.0 * PPQ).round() as u32;
	let mut events: Vec<(u32, bool, u8, u8)> = Vec::new();
	for &(key, velocity, on, off) in notes {
		events.push((ticks(on), true, key, velocity));
		events.push((ticks(off), false, key, 0));
	}
	events.sort_by_key(|&(tick, is_on, ..)| (tick, is_on));
	let mut track = vec![TrackEvent {
		delta: u28::new(0),
		kind: TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000))),
	}];
	let mut now = 0;
	for (tick, is_on, key, velocity) in events {
		let message = if is_on {
			MidiMessage::NoteOn {
				key: u7::new(key),
				vel: u7::new(velocity),
			}
		} else {
			MidiMessage::NoteOff {
				key: u7::new(key),
				vel: u7::new(0),
			}
		};
		track.push(TrackEvent {
			delta: u28::new(tick - now),
			kind: TrackEventKind::Midi {
				channel: u4::new(0),
				message,
			},
		});
		now = tick;
	}
	let last = notes.iter().map(|n| n.3).fold(0.0, f64::max) + 2.5;
	track.push(TrackEvent {
		delta: u28::new(ticks(last).saturating_sub(now)),
		kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
	});
	let smf = Smf {
		header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(PPQ as u16))),
		tracks: vec![track],
	};
	let mut bytes = Vec::new();
	smf.write_std(&mut bytes).unwrap();
	bytes
}

/// The WAV's two channels averaged, as the audio chain hears them.
fn read_mono(path: &Path) -> Vec<f64> {
	let mut reader = hound::WavReader::open(path).unwrap();
	let spec = reader.spec();
	assert_eq!(
		spec.sample_rate, SAMPLE_RATE,
		"the render keeps its sample rate"
	);
	let channels = usize::from(spec.channels);
	let samples: Vec<f64> = reader
		.samples::<i16>()
		.map(|s| f64::from(s.unwrap()) / 32768.0)
		.collect();
	samples
		.chunks(channels)
		.map(|frame| frame.iter().sum::<f64>() / channels as f64)
		.collect()
}

fn hann(i: usize, n: usize) -> f64 {
	0.5 - 0.5 * (2.0 * std::f64::consts::PI * (i as f64 + 0.5) / n as f64).cos()
}

/// The amplitude of the sine at `frequency` in the Hann window centred on `t`.
fn fundamental(samples: &[f64], frequency: f64, t: f64) -> f64 {
	let n = (WINDOW * f64::from(SAMPLE_RATE)) as usize;
	let start = (t * f64::from(SAMPLE_RATE)) as usize - n / 2;
	let (mut re, mut im, mut total) = (0.0, 0.0, 0.0);
	for i in 0..n {
		let w = hann(i, n);
		let phase =
			2.0 * std::f64::consts::PI * frequency * (start + i) as f64 / f64::from(SAMPLE_RATE);
		let x = samples.get(start + i).copied().unwrap_or(0.0) * w;
		re += x * phase.cos();
		im += x * phase.sin();
		total += w;
	}
	2.0 * re.hypot(im) / total
}

/// What [`fundamental`] reads of a sine whose amplitude is `column` (one entry
/// per tick): the amplitude averaged under the same window.
fn windowed(column: &[f64], t: f64) -> f64 {
	let n = (WINDOW * f64::from(TICK_RATE)) as usize;
	let start = (t * f64::from(TICK_RATE)) as usize - n / 2;
	let (mut sum, mut total) = (0.0, 0.0);
	for i in 0..n {
		let w = hann(i, n);
		sum += w * column.get(start + i).copied().unwrap_or(0.0);
		total += w;
	}
	sum / total
}

const _: () = assert!(KEYS == 88);
