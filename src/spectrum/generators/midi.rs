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
/// The controller number of the sustain pedal.
const SUSTAIN_PEDAL: u8 = 64;

/// The fade rate's reference: a SoundFont's decay and release times are how
/// long a fade from the peak to silence would take, and silence is 100 dB down.
const FADE_DB: f64 = 100.0;

/// Below this fraction of the peak a fading note is gone.
fn silent(level: f64) -> bool {
	level <= db_to_amplitude(FADE_DB)
}

fn db_to_amplitude(db: f64) -> f64 {
	10f64.powf(-db / 20.0)
}

/// The key the key-scaling generators count from.
const MIDDLE_KEY: f64 = 60.0;

/// Seconds for a SoundFont time in timecents.
pub fn seconds(timecents: f64) -> f64 {
	(timecents / 1200.0).exp2()
}

/// The SoundFont 2 volume envelope: delay, attack, hold, decay, sustain, release.
///
/// Times are in seconds (a SoundFont's timecents through [`seconds`]) and the
/// sustain is in dB below the peak (its centibels over ten). The attack is a
/// straight line of amplitude; the decay and release fall at a fixed dB per
/// second, so `decay` and `release` are the time a fade from the peak all the
/// way to silence would take.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Envelope {
	/// Silence before the attack starts.
	pub delay: f64,
	/// From silence to the peak.
	pub attack: f64,
	/// At the peak.
	pub hold: f64,
	/// The time a fade from the peak to silence takes; the note falls at that
	/// rate until it reaches the sustain level.
	pub decay: f64,
	/// The level held while the key is, in dB below the peak.
	pub sustain: f64,
	/// The time a fade from the peak to silence takes once the key is let go.
	pub release: f64,
	/// Timecents the hold shortens by per key above middle C
	/// (`GEN_KEYTOVOLENVHOLD`).
	pub key_to_hold: f64,
	/// The same for the decay (`GEN_KEYTOVOLENVDECAY`).
	pub key_to_decay: f64,
}

impl Default for Envelope {
	fn default() -> Self {
		Envelope {
			delay: 0.0,
			attack: 0.005,
			hold: 0.0,
			decay: 5.0,
			sustain: 6.0,
			release: 1.0,
			key_to_hold: 0.0,
			key_to_decay: 0.0,
		}
	}
}

impl Envelope {
	/// A time scaled by the key, `timecents` per key above middle C.
	fn keyed(seconds_at_middle: f64, timecents: f64, key: u8) -> f64 {
		seconds_at_middle * seconds(timecents * (MIDDLE_KEY - f64::from(key)))
	}

	/// The level of a key held for `age` seconds, as a fraction of the peak.
	fn held(&self, age: f64, key: u8) -> f64 {
		let mut t = age - self.delay;
		if t < 0.0 {
			return 0.0;
		}
		if t < self.attack {
			return t / self.attack;
		}
		t -= self.attack;
		let hold = Self::keyed(self.hold, self.key_to_hold, key);
		if t < hold {
			return 1.0;
		}
		t -= hold;
		let decay = Self::keyed(self.decay, self.key_to_decay, key);
		let faded = if decay > 0.0 {
			FADE_DB * t / decay
		} else {
			FADE_DB
		};
		db_to_amplitude(faded.min(self.sustain))
	}

	/// The level of a note `age` seconds old that was let go at `released`.
	fn level(&self, age: f64, released: Option<f64>, key: u8) -> f64 {
		match released {
			Some(released) if age > released => {
				let faded = if self.release > 0.0 {
					FADE_DB * (age - released) / self.release
				} else {
					FADE_DB
				};
				self.held(released, key) * db_to_amplitude(faded)
			}
			_ => self.held(age, key),
		}
	}
}

/// The SoundFont default velocity curve: attenuation of `-40·log₁₀(v / 127)` dB
/// (concave, which is `v²` in amplitude), up to 96 dB, which velocity 0 gets.
fn velocity_amplitude(velocity: f32) -> f64 {
	const MAX_ATTENUATION_DB: f64 = 96.0;
	let raw = (f64::from(velocity) * 127.0).round().clamp(0.0, 127.0);
	let attenuation = if raw == 0.0 {
		MAX_ATTENUATION_DB
	} else {
		(-40.0 * (raw / 127.0).log10()).min(MAX_ATTENUATION_DB)
	};
	db_to_amplitude(attenuation)
}

#[derive(Debug, Clone, Copy)]
struct Voice {
	key: u8,
	/// The amplitude at the envelope's peak.
	peak: f64,
	/// Seconds since the note-on.
	age: f64,
	/// The age at which the note-off came, once it has.
	released: Option<f64>,
	/// Whether the key is up but the sustain pedal is keeping the note on.
	pedaled: bool,
}

/// A voice's index: one per channel and key.
fn slot(channel: u8, key: u8) -> usize {
	usize::from(channel) % CHANNELS * KEYS + usize::from(key) % KEYS
}

/// The notes sounding, one voice per channel and key.
pub struct Voices {
	notes: NoteReader,
	voices: Vec<Option<Voice>>,
	/// Which channels have the sustain pedal down.
	pedals: [bool; CHANNELS],
	pub envelope: Envelope,
	/// Whether a note has ever been played.
	heard: bool,
}

impl Voices {
	pub fn new(envelope: Envelope, notes: NoteReader) -> Self {
		Voices {
			notes,
			voices: vec![None; CHANNELS * KEYS],
			pedals: [false; CHANNELS],
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
					let peak = velocity_amplitude(velocity);
					// Only a note switches the source: hosts send "all notes off"
					// to tracks that never play one.
					self.heard = true;
					self.voices[slot(channel, key)] = Some(Voice {
						key,
						peak,
						age: 0.0,
						released: None,
						pedaled: false,
					});
				}
				NoteEvent::NoteOff { channel, key, .. } => {
					let pedals = self.pedals;
					for (index, voice) in self.matching_indexed(channel, key) {
						let Some(voice) = voice else { continue };
						if pedals[index / KEYS] {
							voice.pedaled = true;
						} else {
							voice.released.get_or_insert(voice.age + dt);
						}
					}
				}
				NoteEvent::MidiCC {
					channel,
					cc: SUSTAIN_PEDAL,
					value,
					..
				} => {
					let channel = usize::from(channel) % CHANNELS;
					let down = value >= 0.5;
					self.pedals[channel] = down;
					if !down {
						let range = channel * KEYS..(channel + 1) * KEYS;
						for voice in self.voices[range].iter_mut().flatten() {
							if std::mem::take(&mut voice.pedaled) {
								voice.released.get_or_insert(voice.age + dt);
							}
						}
					}
				}
				NoteEvent::Choke { channel, key, .. } => {
					self.matching(channel, key).for_each(|voice| *voice = None);
				}
				_ => {}
			}
		}

		let envelope = &self.envelope;
		for slot in &mut self.voices {
			if let Some(voice) = slot {
				voice.age += dt;
				if voice.released.is_some_and(|released| {
					voice.age > released
						&& silent(envelope.level(voice.age, voice.released, voice.key))
				}) {
					*slot = None;
				}
			}
		}
	}

	/// The voices an event addresses. A wildcard channel or key, which is how a
	/// host says "all notes off", addresses every one.
	fn matching(&mut self, channel: Channel, key: Key) -> impl Iterator<Item = &mut Option<Voice>> {
		self.matching_indexed(channel, key).map(|(_, voice)| voice)
	}

	/// [`matching`](Self::matching), with each voice's index.
	fn matching_indexed(
		&mut self,
		channel: Channel,
		key: Key,
	) -> impl Iterator<Item = (usize, &mut Option<Voice>)> {
		let matches =
			|wanted: Option<u8>, value: usize| wanted.is_none_or(|n| usize::from(n) == value);
		self.voices
			.iter_mut()
			.enumerate()
			.filter(move |(index, _)| {
				matches(channel.number(), index / KEYS) && matches(key.number(), index % KEYS)
			})
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
			let amplitude = voice.peak * self.envelope.level(voice.age, voice.released, voice.key);
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

	fn pedal(down: bool) -> NoteEvent<()> {
		NoteEvent::MidiCC {
			timing: 0,
			channel: 0,
			cc: 64,
			value: if down { 1.0 } else { 0.0 },
		}
	}

	fn db(amplitude: f64) -> f64 {
		(20.0 * amplitude.log10() * 1e6).round() / 1e6
	}

	/// An envelope that fades 6 dB a tick, settling at 12 dB down.
	fn six_db_a_tick() -> Envelope {
		Envelope {
			attack: 0.0,
			decay: TICK * FADE_DB / 6.0,
			sustain: 12.0,
			release: TICK * FADE_DB / 6.0,
			..Envelope::default()
		}
	}

	#[test]
	fn a_note_fades_in_decibels_to_its_sustain_then_releases() {
		let (mut writer, reader) = RingBuffer::new(RING_CAPACITY);
		let mut voices = Voices::new(six_db_a_tick(), reader);
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

		writer.push(on(69, 1.0)).unwrap();
		let mut levels = Vec::new();
		for tick in 0..7 {
			if tick == 4 {
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
			levels.push(db(values[12]));
		}

		assert!(voices.heard());
		// Down 6 dB a tick to the 12 dB sustain; the release starts from the end
		// of the note-off's tick and falls 6 dB a tick, until it is silent.
		assert_eq!(levels[..5], [-6.0, -12.0, -12.0, -12.0, -12.0]);
		assert_eq!(levels[5], -18.0);
		assert_eq!(levels[6], -24.0);
	}

	#[test]
	fn delay_attack_and_hold_come_before_the_decay() {
		let envelope = Envelope {
			delay: TICK,
			attack: 2.0 * TICK,
			hold: TICK,
			..six_db_a_tick()
		};
		let at = |age: f64| envelope.level(age, None, 69);
		assert_eq!(at(0.5 * TICK), 0.0);
		assert!(
			(at(2.0 * TICK) - 0.5).abs() < 1e-9,
			"half way up the attack"
		);
		assert_eq!(at(4.0 * TICK), 1.0, "held at the peak");
		assert_eq!(db(at(5.0 * TICK)), -6.0);
	}

	#[test]
	fn keys_above_middle_c_shorten_the_hold_and_decay() {
		let envelope = Envelope {
			hold: 1.0,
			key_to_hold: 1200.0,
			..Envelope::default()
		};
		// An octave of keys (12) at 1200 timecents a key is 2^12 times shorter.
		assert_eq!(
			envelope.level(0.01, None, 72),
			db_to_amplitude((FADE_DB * (0.01 - 0.005 - 1.0 / 4096.0) / 5.0).min(6.0))
		);
		assert_eq!(envelope.level(0.5, None, 60), 1.0);
	}

	#[test]
	fn velocity_follows_the_soundfont_curve() {
		assert_eq!(velocity_amplitude(1.0), 1.0);
		assert!((velocity_amplitude(64.0 / 127.0) - (64.0f64 / 127.0).powi(2)).abs() < 1e-9);
		// Velocity 1 is 40·log₁₀(127) = 84.15 dB down; 0 is the 96 dB floor.
		assert_eq!(db(velocity_amplitude(1.0 / 127.0)), -84.152149);
		assert_eq!(db(velocity_amplitude(0.0)), -96.0);
	}

	#[test]
	fn the_sustain_pedal_holds_a_released_key_until_it_comes_up() {
		let (mut writer, reader) = RingBuffer::new(RING_CAPACITY);
		let mut voices = Voices::new(six_db_a_tick(), reader);
		writer.push(pedal(true)).unwrap();
		writer.push(on(69, 1.0)).unwrap();
		voices.advance(TICK);
		writer.push(off(69)).unwrap();
		for _ in 0..5 {
			voices.advance(TICK);
			assert_eq!(db(drawn(&voices)[12]), -12.0, "still sounding");
		}
		writer.push(pedal(false)).unwrap();
		voices.advance(TICK);
		assert_eq!(db(drawn(&voices)[12]), -12.0, "the release starts here");
		voices.advance(TICK);
		assert_eq!(db(drawn(&voices)[12]), -18.0, "released");
		for _ in 0..20 {
			voices.advance(TICK);
		}
		assert_eq!(drawn(&voices)[12], 0.0);
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
