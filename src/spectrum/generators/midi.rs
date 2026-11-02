//! Spectra drawn from the notes the host plays rather than the sound it makes.
//!
//! Each sounding note is an impulse at its pitch, at the amplitude its velocity
//! and its envelope give it. MIDI notes have no overtones, so nothing but the
//! fundamental is drawn; the diffuser spreads the impulse into something
//! visible.

use nice_plug::midi::{Channel, Key, NoteEvent};
use rtrb::{Consumer, Producer};

use crate::note;
use crate::spectrum::SpectrumParams;

/// The note events between two ticks. A tick is at most a second, and nothing
/// a person plays comes close to a thousand events in one.
pub const RING_CAPACITY: usize = 1024;

/// Where `process` puts the host's note events, and where the spectrum thread
/// reads them.
pub type NoteWriter = Producer<NoteEvent<()>>;
pub type NoteReader = Consumer<NoteEvent<()>>;

const CHANNELS: usize = 16;
const KEYS: usize = 128;

/// An ADSR envelope, in seconds, on a note's amplitude. Each segment is linear.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Envelope {
	/// From silence to the velocity's amplitude.
	pub attack: f64,
	/// From there down to the sustain level.
	pub decay: f64,
	/// The level held while the key is, as a fraction of the peak.
	pub sustain: f64,
	/// From wherever the note was at its note-off down to silence.
	pub release: f64,
}

impl Default for Envelope {
	fn default() -> Self {
		Envelope {
			attack: 0.005,
			decay: 0.4,
			sustain: 0.5,
			release: 0.3,
		}
	}
}

impl Envelope {
	/// The level of a key held for `age` seconds, as a fraction of the peak.
	fn held(&self, age: f64) -> f64 {
		if age < self.attack {
			age / self.attack
		} else if age < self.attack + self.decay {
			1.0 - (1.0 - self.sustain) * (age - self.attack) / self.decay
		} else {
			self.sustain
		}
	}

	/// The level of a note `age` seconds old that was let go at `released`.
	fn level(&self, age: f64, released: Option<f64>) -> f64 {
		match released {
			Some(released) if age > released => {
				self.held(released) * (1.0 - (age - released) / self.release).max(0.0)
			}
			_ => self.held(age),
		}
	}
}

#[derive(Debug, Clone, Copy)]
struct Voice {
	/// The amplitude at the envelope's peak.
	peak: f64,
	/// Seconds since the note-on.
	age: f64,
	/// The age at which the note-off came, once it has.
	released: Option<f64>,
}

/// A voice's index: one per channel and key.
fn slot(channel: u8, key: u8) -> usize {
	usize::from(channel) % CHANNELS * KEYS + usize::from(key) % KEYS
}

/// The notes sounding, one voice per channel and key.
pub struct Voices {
	notes: NoteReader,
	voices: Vec<Option<Voice>>,
	pub envelope: Envelope,
	/// Whether a note has ever been played.
	heard: bool,
}

impl Voices {
	pub fn new(envelope: Envelope, notes: NoteReader) -> Self {
		Voices {
			notes,
			voices: vec![None; CHANNELS * KEYS],
			envelope,
			heard: false,
		}
	}

	/// Whether a note has ever been played.
	pub fn heard(&self) -> bool {
		self.heard
	}

	/// Takes the events that arrived since the last tick, then moves every
	/// voice `dt` seconds on.
	///
	/// The events are timed by the tick they arrive in, not by the sample they
	/// carry. A note-on counts from the start of the tick's interval and a
	/// note-off from its end, so a note shorter than a tick still sounds for
	/// one, rather than going on and off before it is ever drawn.
	pub fn advance(&mut self, dt: f64) {
		while let Ok(event) = self.notes.pop() {
			match event {
				NoteEvent::NoteOn {
					channel: Channel::Number(channel),
					key: Key::Number(key),
					velocity,
					..
				} => {
					// The DLS and General MIDI 2 default curve: 40·log₁₀(v) dB,
					// which is v² in amplitude.
					let peak = f64::from(velocity).powi(2);
					// Only a note switches the source: hosts send "all notes off"
					// to tracks that never play one.
					self.heard = true;
					self.voices[slot(channel, key)] = Some(Voice {
						peak,
						age: 0.0,
						released: None,
					});
				}
				NoteEvent::NoteOff { channel, key, .. } => {
					for voice in self.matching(channel, key).flatten() {
						voice.released.get_or_insert(voice.age + dt);
					}
				}
				NoteEvent::Choke { channel, key, .. } => {
					self.matching(channel, key).for_each(|voice| *voice = None);
				}
				_ => {}
			}
		}

		let release = self.envelope.release;
		for slot in &mut self.voices {
			if let Some(voice) = slot {
				voice.age += dt;
				if voice
					.released
					.is_some_and(|released| voice.age - released > release)
				{
					*slot = None;
				}
			}
		}
	}

	/// The voices an event addresses. A wildcard channel or key, which is how a
	/// host says "all notes off", addresses every one.
	fn matching(&mut self, channel: Channel, key: Key) -> impl Iterator<Item = &mut Option<Voice>> {
		let matches =
			|wanted: Option<u8>, value: usize| wanted.is_none_or(|n| usize::from(n) == value);
		self.voices
			.iter_mut()
			.enumerate()
			.filter(move |(index, _)| {
				matches(channel.number(), index / KEYS) && matches(key.number(), index % KEYS)
			})
			.map(|(_, voice)| voice)
	}

	/// Draws the sounding voices onto `values`, a spectrum on the grid `params`
	/// describes, in amplitude.
	///
	/// A note between two bins splits its power between them linearly, and
	/// voices on the same bin add in power, as the audio generator's partials
	/// do.
	pub fn fill(&self, values: &mut [f64], params: &SpectrumParams) {
		values.fill(0.0);
		let (Some(low), Some(high)) = (params.min_log_freq(), params.max_log_freq()) else {
			return;
		};
		let last = values.len() - 1;
		for (index, voice) in self.voices.iter().enumerate() {
			let Some(voice) = voice else { continue };
			let amplitude = voice.peak * self.envelope.level(voice.age, voice.released);
			let key = (index % KEYS) as isize;
			let log_freq = (note!(A, 4) + (key - 69)).log_frequency();
			let position = (log_freq - low) / (high - low) * last as f64;
			if !(0.0..=last as f64).contains(&position) {
				continue;
			}
			let bin = (position as usize).min(last - 1);
			let above = position - bin as f64;
			let power = amplitude * amplitude;
			values[bin] += power * (1.0 - above);
			values[bin + 1] += power * above;
		}
		for value in values {
			*value = value.sqrt();
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use nice_plug::midi::VoiceID;
	use rtrb::RingBuffer;

	const TICK: f64 = 0.02;

	fn on(key: u8, velocity: f32) -> NoteEvent<()> {
		NoteEvent::NoteOn {
			timing: 0,
			voice_id: VoiceID::Wildcard,
			channel: Channel::Number(0),
			key: Key::Number(key),
			velocity,
		}
	}

	fn off(key: u8) -> NoteEvent<()> {
		NoteEvent::NoteOff {
			timing: 0,
			voice_id: VoiceID::Wildcard,
			channel: Channel::Number(0),
			key: Key::Number(key),
			velocity: 0.0,
		}
	}

	/// The grid from A3 to A5 at 12 bins an octave: one bin a semitone, so
	/// every note lands on a bin and A4 is bin 12.
	fn grid() -> SpectrumParams {
		SpectrumParams::exp_spaced(25, 220.0, 880.0)
	}

	fn drawn(voices: &Voices) -> Vec<f64> {
		let mut values = vec![0.0; 25];
		voices.fill(&mut values, &grid());
		values
	}

	#[test]
	fn a_note_follows_its_envelope_at_the_square_of_its_velocity() {
		let (mut writer, reader) = RingBuffer::new(RING_CAPACITY);
		let envelope = Envelope {
			attack: 0.0,
			decay: 2.0 * TICK,
			sustain: 0.5,
			release: 2.0 * TICK,
		};
		let mut voices = Voices::new(envelope, reader);
		writer
			.push(NoteEvent::Choke {
				timing: 0,
				voice_id: VoiceID::Wildcard,
				channel: Channel::Wildcard,
				key: Key::Wildcard,
			})
			.unwrap();
		voices.advance(TICK);
		assert!(!voices.heard(), "all notes off is not a note");

		writer.push(on(69, 0.5)).unwrap();
		let mut levels = Vec::new();
		for tick in 0..6 {
			if tick == 3 {
				writer.push(off(69)).unwrap();
			}
			voices.advance(TICK);
			let values = drawn(&voices);
			assert!(
				values
					.iter()
					.enumerate()
					.all(|(bin, &v)| bin == 12 || v == 0.0),
				"only A4's bin sounds: {values:?}",
			);
			levels.push((values[12] * 1e9).round() / 1e9);
		}

		assert!(voices.heard());
		// Peak 0.25; half way down the decay, then the sustain, then the
		// release from the end of the note-off's tick.
		assert_eq!(levels, [0.1875, 0.125, 0.125, 0.125, 0.0625, 0.0]);
	}

	#[test]
	fn a_note_shorter_than_a_tick_still_sounds_for_one() {
		let (mut writer, reader) = RingBuffer::new(RING_CAPACITY);
		let envelope = Envelope {
			attack: TICK,
			release: 0.0,
			..Envelope::default()
		};
		let mut voices = Voices::new(envelope, reader);

		writer.push(on(69, 1.0)).unwrap();
		writer.push(off(69)).unwrap();
		voices.advance(TICK);

		assert_eq!(drawn(&voices)[12], 1.0, "it reaches the end of its attack");
	}

	#[test]
	fn a_note_between_two_bins_splits_its_power() {
		let (mut writer, reader) = RingBuffer::new(RING_CAPACITY);
		let mut voices = Voices::new(Envelope::default(), reader);
		writer.push(on(69, 1.0)).unwrap();
		voices.advance(Envelope::default().attack);

		// Two bins, a whole tone apart, with A4 half way between them.
		let mut values = vec![0.0; 2];
		let grid =
			SpectrumParams::exp_spaced(2, note!(Ab, 4).frequency(), note!(Bb, 4).frequency());
		voices.fill(&mut values, &grid);
		let power = values[0].powi(2) + values[1].powi(2);
		assert!((values[0] - values[1]).abs() < 1e-9, "{values:?}");
		assert!((power - 1.0).abs() < 1e-9, "the power is kept: {power}");
	}
}
