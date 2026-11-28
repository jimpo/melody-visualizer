//! The reference the audio chain is scored against, and the score.
//!
//! The reference is the MIDI generator's own output: each sounding note is its
//! fundamental alone, at the amplitude the SoundFont's zones give it, with no
//! transforms. The diffuser and the rest of the chain are taste, not truth, so
//! they are left out. It runs on the eval runner's ticks (tick `k` ends at
//! `k / update_rate` seconds), so row `k` of the reference lines up with row `k`
//! of the chain.
//!
//! Both sides are folded into the 88 piano keys ([`fold`]) before comparing, so
//! a change to the chain's resolution does not change the metric.
//!
//! The chain's latency is measured, not assumed: [`align`] cross-correlates the
//! two over a range of offsets and scores at the best one. The offset is the
//! latency, and is reported.

use std::sync::Arc;

use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};
use nice_plug::midi::{Channel, Key, NoteEvent, VoiceID};
use rtrb::RingBuffer;

use crate::spectrum::SpectrumParams;
use crate::spectrum::generators::midi::Voices;
use crate::spectrum::generators::soundfont::Bank;

/// The piano's keys, A0 to C8: MIDI notes 21 to 108.
pub const KEYS: usize = 88;
const LOWEST_KEY: u8 = 21;

/// A note event and the time it happens, in seconds from the start of the file.
pub type Timed = (f64, NoteEvent<()>);

/// Reads the notes, pedal and channel events of a Standard MIDI File, with the
/// file's tempo changes applied. The events come back sorted by time.
pub fn read_midi(bytes: &[u8]) -> Result<Vec<Timed>, String> {
	let smf = Smf::parse(bytes).map_err(|e| e.to_string())?;
	let tpq = match smf.header.timing {
		Timing::Metrical(t) => f64::from(t.as_int()),
		Timing::Timecode(fps, sub) => {
			// Absolute time: ticks are fixed fractions of a second.
			return Ok(sorted(collect(&smf, |tick, _| {
				tick / (f64::from(fps.as_f32()) * f64::from(sub))
			})));
		}
	};

	// The tempo map: (tick, microseconds per quarter) in order, across all tracks.
	let mut tempos: Vec<(u64, f64)> = vec![(0, 500_000.0)];
	for track in &smf.tracks {
		let mut tick = 0u64;
		for event in track {
			tick += u64::from(event.delta.as_int());
			if let TrackEventKind::Meta(MetaMessage::Tempo(us)) = event.kind {
				tempos.push((tick, f64::from(us.as_int())));
			}
		}
	}
	tempos.sort_by_key(|&(tick, _)| tick);
	// Seconds at the start of each tempo segment.
	let mut starts = Vec::with_capacity(tempos.len());
	let mut seconds = 0.0;
	for (i, &(tick, us)) in tempos.iter().enumerate() {
		if let Some(&(prev_tick, prev_us)) = i.checked_sub(1).map(|j| &tempos[j]) {
			seconds += (tick - prev_tick) as f64 * prev_us / tpq / 1e6;
			let _ = us;
		}
		starts.push(seconds);
	}
	let to_seconds = |tick: f64, _: ()| {
		let i = tempos
			.partition_point(|&(t, _)| (t as f64) <= tick)
			.saturating_sub(1);
		starts[i] + (tick - tempos[i].0 as f64) * tempos[i].1 / tpq / 1e6
	};
	Ok(sorted(collect(&smf, to_seconds)))
}

fn sorted(mut events: Vec<Timed>) -> Vec<Timed> {
	events.sort_by(|a, b| a.0.total_cmp(&b.0));
	events
}

fn collect<F: Fn(f64, ()) -> f64>(smf: &Smf, to_seconds: F) -> Vec<Timed> {
	let mut events = Vec::new();
	for track in &smf.tracks {
		let mut tick = 0u64;
		for event in track {
			tick += u64::from(event.delta.as_int());
			let TrackEventKind::Midi { channel, message } = event.kind else {
				continue;
			};
			let channel = channel.as_int();
			let key = |k: midly::num::u7| Key::Number(k.as_int());
			let note = match message {
				// A note-on at velocity 0 is a note-off.
				MidiMessage::NoteOn { key: k, vel } if vel.as_int() == 0 => NoteEvent::NoteOff {
					timing: 0,
					voice_id: VoiceID::Wildcard,
					channel: Channel::Number(channel),
					key: key(k),
					velocity: 0.0,
				},
				MidiMessage::NoteOn { key: k, vel } => NoteEvent::NoteOn {
					timing: 0,
					voice_id: VoiceID::Wildcard,
					channel: Channel::Number(channel),
					key: key(k),
					velocity: f32::from(vel.as_int()) / 127.0,
				},
				MidiMessage::NoteOff { key: k, vel } => NoteEvent::NoteOff {
					timing: 0,
					voice_id: VoiceID::Wildcard,
					channel: Channel::Number(channel),
					key: key(k),
					velocity: f32::from(vel.as_int()) / 127.0,
				},
				MidiMessage::Controller { controller, value } => NoteEvent::MidiCC {
					timing: 0,
					channel,
					cc: controller.as_int(),
					value: f32::from(value.as_int()) / 127.0,
				},
				_ => continue,
			};
			events.push((to_seconds(tick as f64, ()), note));
		}
	}
	events
}

/// The reference spectrum, one row of 88 key amplitudes per tick.
///
/// Tick `k` (counting from 1, as the eval runner does) covers the time
/// `((k-1)/update_rate, k/update_rate]`; the events in it are applied, the
/// voices aged one tick, and the row drawn. `ticks` rows are returned.
pub fn render(
	events: &[Timed],
	bank: &Arc<Bank>,
	update_rate: u32,
	ticks: usize,
) -> Vec<[f64; KEYS]> {
	let rate = f64::from(update_rate.max(1));
	// Room for every event of the busiest tick.
	let mut per_tick = std::collections::HashMap::<usize, usize>::new();
	for (time, _) in events {
		*per_tick.entry(tick_of(*time, rate)).or_default() += 1;
	}
	let capacity = per_tick.values().copied().max().unwrap_or(0).max(1);
	let (mut writer, reader) = RingBuffer::new(capacity);
	let mut voices = Voices::new(Arc::clone(bank), reader);

	// One bin per key, so the generator's impulse lands exactly on its key.
	let low = key_frequency(0);
	let high = key_frequency(KEYS - 1);
	let params = SpectrumParams::exp_spaced(KEYS, low, high);

	let mut rows = Vec::with_capacity(ticks);
	let mut next = 0;
	for tick in 1..=ticks {
		while next < events.len() && tick_of(events[next].0, rate) <= tick {
			// Cannot be full: the ring holds the busiest tick's events.
			let _ = writer.push(events[next].1);
			next += 1;
		}
		voices.advance(1.0 / rate);
		let mut values = vec![0.0; KEYS];
		voices.fill(&mut values, &params);
		rows.push(fold(&values, &params));
	}
	rows
}

/// The 1-based tick whose interval `(.., k / rate]` holds `time`.
fn tick_of(time: f64, rate: f64) -> usize {
	((time * rate - 1e-9).ceil() as usize).max(1)
}

/// The frequency of piano key `index` (0 is A0).
fn key_frequency(index: usize) -> f64 {
	440.0 * ((f64::from(LOWEST_KEY) + index as f64 - 69.0) / 12.0).exp2()
}

/// Folds a spectrum on the grid `params` into the 88 piano keys.
///
/// Each key takes the **max** over the bins within ±½ semitone of its pitch.
/// The max, not the sum, because a sum counts bins: the same tone would read
/// louder on a finer grid, and the metric would move with the chain's
/// resolution. The max of a tone's peak does not depend on how many bins
/// surround it. (A tone between two bins is split between them, so a coarse
/// grid still reads somewhat low; the max keeps that to the one bin's share.)
/// A key whose band holds no bin, on a grid coarser than a semitone, takes the
/// nearest bin.
pub fn fold(values: &[f64], params: &SpectrumParams) -> [f64; KEYS] {
	let logs = params.log_frequencies();
	let mut out = [0.0; KEYS];
	for (key, slot) in out.iter_mut().enumerate() {
		let centre = key_frequency(key).log2();
		let (lo, hi) = (centre - 1.0 / 24.0, centre + 1.0 / 24.0);
		let start = logs.partition_point(|&l| l < lo - 1e-9);
		let end = logs.partition_point(|&l| l <= hi + 1e-9);
		*slot = if start < end {
			values[start..end].iter().copied().fold(0.0, f64::max)
		} else if logs.is_empty() {
			0.0
		} else {
			let i = start.min(logs.len() - 1);
			let j = i.saturating_sub(1);
			let nearest = if (logs[i] - centre).abs() < (logs[j] - centre).abs() {
				i
			} else {
				j
			};
			values[nearest]
		};
	}
	out
}

/// The result of aligning the chain to the reference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Alignment {
	/// How many ticks the chain trails the reference: output tick `k + lag`
	/// matches reference tick `k`. Negative if it leads.
	pub lag: isize,
	/// Normalised correlation at that lag, in [0, 1] for non-negative spectra;
	/// 1 is a perfect match up to scale.
	pub score: f64,
}

/// Cross-correlates `output` with `reference` at every lag in
/// `-max_lag..=max_lag` ticks and returns the best.
///
/// The correlation sums over keys and ticks, normalised by the energy of the
/// overlapping parts, so it ignores overall level (the chain's gain is not the
/// reference's) and is not biased towards lags with a short overlap. Returns
/// `None` if either side is silent at every lag.
pub fn align(
	reference: &[[f64; KEYS]],
	output: &[[f64; KEYS]],
	max_lag: usize,
) -> Option<Alignment> {
	let mut best: Option<Alignment> = None;
	for lag in -(max_lag as isize)..=max_lag as isize {
		let (mut dot, mut rr, mut oo) = (0.0, 0.0, 0.0);
		for (k, r) in reference.iter().enumerate() {
			let Some(o) = usize::try_from(k as isize + lag)
				.ok()
				.and_then(|i| output.get(i))
			else {
				continue;
			};
			for key in 0..KEYS {
				dot += r[key] * o[key];
				rr += r[key] * r[key];
				oo += o[key] * o[key];
			}
		}
		let norm = (rr * oo).sqrt();
		if norm <= 0.0 {
			continue;
		}
		let score = dot / norm;
		if best.is_none_or(|b| score > b.score) {
			best = Some(Alignment { lag, score });
		}
	}
	best
}

#[cfg(test)]
mod tests {
	use super::*;

	fn row(key: usize, v: f64) -> [f64; KEYS] {
		let mut r = [0.0; KEYS];
		r[key] = v;
		r
	}

	#[test]
	fn fold_takes_the_max_in_the_band_whatever_the_grid() {
		// 4 bins per semitone around A4 (key 48); a tone in one bin.
		let fine = SpectrumParams::exp_spaced(4 * 87 + 1, key_frequency(0), key_frequency(87));
		let mut values = vec![0.0; fine.samples()];
		values[4 * 48 + 1] = 0.5; // a quarter semitone above A4
		values[4 * 48 + 3] = 0.9; // beyond +½: the next key's
		let folded = fold(&values, &fine);
		assert_eq!(folded[48], 0.5);
		assert_eq!(folded[49], 0.9);
		assert_eq!(folded[47], 0.0);
	}

	#[test]
	fn a_key_between_coarse_bins_takes_the_nearest() {
		let coarse = SpectrumParams::exp_spaced(8, 110.0, 1760.0);
		let values: Vec<f64> = (1..=8).map(f64::from).collect();
		let folded = fold(&values, &coarse);
		assert!(folded.iter().all(|&v| v == 0.0 || (1.0..=8.0).contains(&v)));
		assert!(folded.iter().any(|&v| v > 0.0));
	}

	#[test]
	fn align_recovers_the_offset_and_a_perfect_score() {
		let reference: Vec<_> = (0..40)
			.map(|k| row(k % 5 + 10, 1.0 + (k % 3) as f64))
			.collect();
		let delay = 7;
		let output: Vec<_> = (0..40)
			.map(|k| {
				if k >= delay {
					reference[k - delay].map(|v| v * 0.3)
				} else {
					[0.0; KEYS]
				}
			})
			.collect();
		let a = align(&reference, &output, 20).unwrap();
		assert_eq!(a.lag, delay as isize);
		assert!((a.score - 1.0).abs() < 1e-9, "{}", a.score);
	}

	#[test]
	fn align_of_silence_is_none() {
		assert_eq!(align(&[[0.0; KEYS]; 4], &[[0.0; KEYS]; 4], 2), None);
	}

	#[test]
	fn a_rendered_note_sits_on_its_key_and_fades() {
		let on = NoteEvent::NoteOn {
			timing: 0,
			voice_id: VoiceID::Wildcard,
			channel: Channel::Number(0),
			key: Key::Number(69),
			velocity: 1.0,
		};
		let off = NoteEvent::NoteOff {
			timing: 0,
			voice_id: VoiceID::Wildcard,
			channel: Channel::Number(0),
			key: Key::Number(69),
			velocity: 0.0,
		};
		let events = [(0.0, on), (0.5, off)];
		let rows = render(&events, &Arc::new(Bank::fallback()), 50, 100);
		let a4 = usize::from(69 - LOWEST_KEY);
		assert!(rows[0][a4] > 0.0);
		// The zone's tuning puts a few cents of the note on the key below.
		for (key, v) in rows[10].iter().enumerate() {
			let near = key.abs_diff(a4) == 1;
			assert!(
				key == a4 || *v < if near { 0.25 * rows[10][a4] } else { 1e-6 },
				"key {key} = {v}"
			);
		}
		assert!(rows[99][a4] < rows[10][a4] * 0.01, "released note fades");
	}

	#[test]
	fn reads_a_midi_file_with_tempo() {
		// Format 0, 480 tpq, 120 bpm default: a note-on at tick 0, off at 480
		// (0.5 s), then a tempo of 1 s per quarter and a note at tick 960.
		let mut track = vec![0x00, 0x90, 60, 100, 0x83, 0x60, 0x80, 60, 0];
		track.extend([0x00, 0xFF, 0x51, 0x03, 0x0F, 0x42, 0x40]); // tempo 1_000_000
		track.extend([0x83, 0x60, 0x90, 62, 100, 0x00, 0xFF, 0x2F, 0x00]);
		let mut file = b"MThd".to_vec();
		file.extend([0, 0, 0, 6, 0, 0, 0, 1, 0x01, 0xE0]);
		file.extend(b"MTrk");
		file.extend((track.len() as u32).to_be_bytes());
		file.extend(&track);
		let events = read_midi(&file).unwrap();
		assert_eq!(events.len(), 3);
		assert!((events[1].0 - 0.5).abs() < 1e-9);
		assert!((events[2].0 - 1.5).abs() < 1e-9);
	}
}
