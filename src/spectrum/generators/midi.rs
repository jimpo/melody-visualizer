//! Spectra drawn from the notes the host plays rather than the sound it makes.
//!
//! Each sounding note is an impulse at its pitch, at the amplitude its velocity
//! and its envelope give it. MIDI notes have no overtones, so nothing but the
//! fundamental is drawn; the diffuser spreads the impulse into something
//! visible.

use std::sync::Arc;

use nice_plug::midi::{Channel, Key, NoteEvent};
use rtrb::{Consumer, Producer};

use super::soundfont::Bank;

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
/// The controller numbers the generator reads.
const DATA_ENTRY_MSB: u8 = 6;
const VOLUME: u8 = 7;
const EXPRESSION: u8 = 11;
const DATA_ENTRY_LSB: u8 = 38;
const SUSTAIN_PEDAL: u8 = 64;
const SOSTENUTO_PEDAL: u8 = 66;
const RPN_LSB: u8 = 100;
const RPN_MSB: u8 = 101;
/// The registered parameter that sets the pitch bend range.
const PITCH_BEND_RANGE: (u8, u8) = (0, 0);
/// A registered parameter number that selects nothing.
const RPN_NULL: (u8, u8) = (127, 127);

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
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// The attenuation of a 7-bit value on the SoundFont's default concave curve,
/// `-40·log₁₀(v / 127)` dB, up to 96 dB, which 0 gets. FluidSynth uses it for
/// velocity, CC7 and CC11 alike.
fn concave_attenuation(raw: u8) -> f64 {
	const MAX_ATTENUATION_DB: f64 = 96.0;
	if raw == 0 {
		MAX_ATTENUATION_DB
	} else {
		(-40.0 * (f64::from(raw) / 127.0).log10()).min(MAX_ATTENUATION_DB)
	}
}

/// A 0..=1 value from the host as the 7-bit number it carries.
fn seven_bit(value: f32) -> u8 {
	(f64::from(value) * 127.0).round().clamp(0.0, 127.0) as u8
}

/// The partial one zone gives a note.
#[derive(Debug, Clone, Copy)]
struct Layer {
	envelope: Envelope,
	/// The amplitude at the envelope's peak: velocity and the zone's attenuation.
	peak: f64,
	/// The pitch in cents above MIDI key 0, before pitch bend.
	cents: f64,
}

impl Layer {
	/// Whether the layer is past its start and too quiet to draw again.
	fn gone(&self, age: f64, released: Option<f64>, key: u8) -> bool {
		age > self.envelope.delay + self.envelope.attack
			&& silent(self.envelope.level(age, released, key))
	}
}

#[derive(Debug, Clone)]
struct Voice {
	channel: u8,
	key: u8,
	/// One layer per zone that plays: their levels add.
	layers: Vec<Layer>,
	/// Seconds since the note-on.
	age: f64,
	/// The age at which the note ends, once it has.
	released: Option<f64>,
	/// Whether the key is up.
	key_up: bool,
	/// Whether the sostenuto pedal is holding the note.
	sostenuto: bool,
}

/// What a channel's controllers have set.
#[derive(Debug, Clone, Copy)]
struct ChannelState {
	sustain: bool,
	sostenuto: bool,
	/// CC7 and CC11, 7 bits.
	volume: u8,
	expression: u8,
	/// The pitch wheel, -1 to 1.
	bend: f64,
	/// The bend at the wheel's end, in semitones.
	bend_range: f64,
	/// The registered parameter selected, and the data entry's two bytes.
	rpn: (u8, u8),
	data: (u8, u8),
}

impl Default for ChannelState {
	fn default() -> Self {
		ChannelState {
			sustain: false,
			sostenuto: false,
			volume: 100,
			expression: 127,
			bend: 0.0,
			bend_range: 2.0,
			rpn: RPN_NULL,
			data: (0, 0),
		}
	}
}

impl ChannelState {
	/// The amplitude CC7 and CC11 give every note on the channel.
	fn gain(&self) -> f64 {
		db_to_amplitude(concave_attenuation(self.volume) + concave_attenuation(self.expression))
	}

	/// The pitch bend in cents.
	fn bend_cents(&self) -> f64 {
		self.bend * self.bend_range * 100.0
	}

	fn control(&mut self, cc: u8, value: u8) {
		match cc {
			SUSTAIN_PEDAL => self.sustain = value >= 64,
			SOSTENUTO_PEDAL => self.sostenuto = value >= 64,
			VOLUME => self.volume = value,
			EXPRESSION => self.expression = value,
			RPN_MSB => self.rpn.0 = value,
			RPN_LSB => self.rpn.1 = value,
			DATA_ENTRY_MSB | DATA_ENTRY_LSB => {
				if cc == DATA_ENTRY_MSB {
					self.data.0 = value;
				} else {
					self.data.1 = value;
				}
				if self.rpn == PITCH_BEND_RANGE {
					// Semitones, plus hundredths of one in the second byte.
					self.bend_range = f64::from(self.data.0) + f64::from(self.data.1) / 100.0;
				}
			}
			_ => {}
		}
	}
}

/// The notes sounding.
///
/// Each note-on is one voice, made of a layer for every zone of the preset that
/// plays it. Pressing a key again does not cut the voice before it: that one is
/// released and fades.
pub struct Voices {
	notes: NoteReader,
	voices: Vec<Voice>,
	channels: [ChannelState; CHANNELS],
	bank: Arc<Bank>,
	/// Whether a note has ever been played.
	heard: bool,
}

impl Voices {
	pub fn new(bank: Arc<Bank>, notes: NoteReader) -> Self {
		Voices {
			notes,
			voices: Vec::new(),
			channels: [ChannelState::default(); CHANNELS],
			bank,
			heard: false,
		}
	}

	/// Plays the notes struck from now on with `bank`.
	pub fn set_bank(&mut self, bank: Arc<Bank>) {
		self.bank = bank;
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
					// Only a note switches the source: hosts send "all notes off"
					// to tracks that never play one.
					self.heard = true;
					// The key struck again lets go of the note before it.
					self.key_up(Channel::Number(channel), Key::Number(key));
					let raw = seven_bit(velocity);
					let velocity_db = concave_attenuation(raw);
					let layers: Vec<Layer> = self
						.bank
						.zones(0, 0, key, raw)
						.map(|zone| Layer {
							envelope: zone.envelope,
							peak: db_to_amplitude(velocity_db + zone.attenuation),
							cents: zone.cents(key),
						})
						.collect();
					self.voices.push(Voice {
						channel: channel % CHANNELS as u8,
						key,
						layers,
						age: 0.0,
						released: None,
						key_up: false,
						sostenuto: false,
					});
				}
				NoteEvent::NoteOff { channel, key, .. } => self.key_up(channel, key),
				NoteEvent::MidiCC {
					channel, cc, value, ..
				} => {
					let index = usize::from(channel) % CHANNELS;
					self.channels[index].control(cc, seven_bit(value));
					if cc == SOSTENUTO_PEDAL {
						// The pedal holds the notes whose keys are down when it
						// goes down, and lets them go when it comes up.
						let down = self.channels[index].sostenuto;
						for voice in &mut self.voices {
							if usize::from(voice.channel) == index {
								voice.sostenuto = down && !voice.key_up;
							}
						}
					}
				}
				NoteEvent::MidiPitchBend { channel, value, .. } => {
					self.channels[usize::from(channel) % CHANNELS].bend =
						(f64::from(value) - 0.5) * 2.0;
				}
				NoteEvent::Choke { channel, key, .. } => {
					self.voices.retain(|voice| !addressed(voice, channel, key));
				}
				_ => {}
			}
			self.settle(dt);
		}

		for voice in &mut self.voices {
			voice.age += dt;
		}
		self.voices.retain(|voice| {
			!voice
				.layers
				.iter()
				.all(|layer| layer.gone(voice.age, voice.released, voice.key))
		});
	}

	/// Lets go of the keys an event addresses. A wildcard channel or key, which
	/// is how a host says "all notes off", addresses every one.
	fn key_up(&mut self, channel: Channel, key: Key) {
		for voice in &mut self.voices {
			if addressed(voice, channel, key) {
				voice.key_up = true;
			}
		}
	}

	/// Starts the release of every voice nothing holds any more: its key up, and
	/// neither the sustain pedal nor the sostenuto pedal on it.
	fn settle(&mut self, dt: f64) {
		for voice in &mut self.voices {
			let pedaled = self.channels[usize::from(voice.channel)].sustain;
			if voice.key_up && !voice.sostenuto && !pedaled {
				voice.released.get_or_insert(voice.age + dt);
			}
		}
	}

	/// Draws the sounding voices onto `values`, a spectrum on the grid `params`
	/// describes, in amplitude.
	///
	/// A note between two bins splits its power between them linearly, and
	/// layers and voices on the same bin add in power, as the audio generator's
	/// partials do.
	pub fn fill(&self, values: &mut [f64], params: &SpectrumParams) {
		values.fill(0.0);
		let (Some(low), Some(high)) = (params.min_log_freq(), params.max_log_freq()) else {
			return;
		};
		let last = values.len() - 1;
		let a4 = note!(A, 4).log_frequency();
		for voice in &self.voices {
			let channel = &self.channels[usize::from(voice.channel)];
			let gain = channel.gain();
			let bend = channel.bend_cents();
			for layer in &voice.layers {
				let amplitude =
					gain * layer.peak * layer.envelope.level(voice.age, voice.released, voice.key);
				let log_freq = a4 + (layer.cents + bend - 6900.0) / 1200.0;
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
		}
		for value in values {
			*value = value.sqrt();
		}
	}
}

/// Whether an event for `channel` and `key` addresses `voice`.
fn addressed(voice: &Voice, channel: Channel, key: Key) -> bool {
	channel
		.number()
		.is_none_or(|n| usize::from(n) % CHANNELS == usize::from(voice.channel))
		&& key.number().is_none_or(|n| n == voice.key)
}

#[cfg(test)]
mod tests {
	use super::*;

	use nice_plug::midi::VoiceID;
	use rtrb::RingBuffer;

	use super::super::soundfont::Zone;

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

	fn cc(cc: u8, value: u8) -> NoteEvent<()> {
		NoteEvent::MidiCC {
			timing: 0,
			channel: 0,
			cc,
			value: f32::from(value) / 127.0,
		}
	}

	/// A bank whose one preset has a zone with `envelope` for every key.
	fn bank(envelope: Envelope) -> Arc<Bank> {
		Arc::new(Bank::from_zones(vec![Zone::new(envelope)]))
	}

	/// Voices playing `bank`, with the channel volume at its full 127 so a note
	/// at full velocity draws at amplitude 1.
	fn voices_of(bank: Arc<Bank>) -> (NoteWriter, Voices) {
		let (mut writer, reader) = RingBuffer::new(RING_CAPACITY);
		writer.push(cc(VOLUME, 127)).unwrap();
		(writer, Voices::new(bank, reader))
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
		let (mut writer, mut voices) = voices_of(bank(six_db_a_tick()));
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
		let amplitude = |raw| db_to_amplitude(concave_attenuation(raw));
		assert_eq!(amplitude(127), 1.0);
		assert!((amplitude(64) - (64.0f64 / 127.0).powi(2)).abs() < 1e-9);
		// Velocity 1 is 40·log₁₀(127) = 84.15 dB down; 0 is the 96 dB floor.
		assert_eq!(db(amplitude(1)), -84.152149);
		assert_eq!(db(amplitude(0)), -96.0);
	}

	#[test]
	fn the_sustain_pedal_holds_a_released_key_until_it_comes_up() {
		let (mut writer, mut voices) = voices_of(bank(six_db_a_tick()));
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
		let envelope = Envelope {
			attack: TICK,
			release: 0.0,
			..Envelope::default()
		};
		let (mut writer, mut voices) = voices_of(bank(envelope));

		writer.push(on(69, 1.0)).unwrap();
		writer.push(off(69)).unwrap();
		voices.advance(TICK);

		assert_eq!(drawn(&voices)[12], 1.0, "it reaches the end of its attack");
	}

	#[test]
	fn a_note_between_two_bins_splits_its_power() {
		let (mut writer, mut voices) = voices_of(bank(Envelope::default()));
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

	fn at_a4(voices: &Voices) -> f64 {
		drawn(voices)[12]
	}

	#[test]
	fn the_zones_that_play_each_make_a_voice_and_their_levels_add() {
		let mut left = Zone::new(six_db_a_tick());
		left.attenuation = 6.0;
		let right = Zone::new(six_db_a_tick());
		let (mut writer, reader) = RingBuffer::new(RING_CAPACITY);
		writer.push(cc(VOLUME, 127)).unwrap();
		let mut voices = Voices::new(Arc::new(Bank::from_zones(vec![left, right])), reader);
		writer.push(on(69, 1.0)).unwrap();
		voices.advance(0.0);
		// Both on one bin add in power: 1 + (1/2)² = 1.25.
		let expected = (1.0f64 + db_to_amplitude(6.0).powi(2)).sqrt();
		assert!((at_a4(&voices) - expected).abs() < 1e-9);
	}

	#[test]
	fn striking_a_sounding_key_releases_the_old_voice_instead_of_cutting_it() {
		let (mut writer, mut voices) = voices_of(bank(six_db_a_tick()));
		writer.push(on(69, 1.0)).unwrap();
		voices.advance(TICK);
		writer.push(on(69, 1.0)).unwrap();
		voices.advance(TICK);
		// The old voice is at -18 dB and falling, the new at -6: they add.
		let expected = (db_to_amplitude(12.0).powi(2) + db_to_amplitude(6.0).powi(2)).sqrt();
		assert!(
			(at_a4(&voices) - expected).abs() < 1e-9,
			"{}",
			at_a4(&voices)
		);
		for _ in 0..40 {
			voices.advance(TICK);
		}
		assert_eq!(db(at_a4(&voices)), -12.0, "only the held voice is left");
	}

	#[test]
	fn the_sostenuto_pedal_holds_only_the_notes_down_when_it_goes_down() {
		let (mut writer, mut voices) = voices_of(bank(six_db_a_tick()));
		writer.push(on(69, 1.0)).unwrap();
		writer.push(cc(SOSTENUTO_PEDAL, 127)).unwrap();
		writer.push(on(70, 1.0)).unwrap();
		voices.advance(TICK);
		writer.push(off(69)).unwrap();
		writer.push(off(70)).unwrap();
		for _ in 0..40 {
			voices.advance(TICK);
		}
		let values = drawn(&voices);
		assert_eq!(db(values[12]), -12.0, "A4 was down: held");
		assert_eq!(values[13], 0.0, "A#4 came after: released");
		writer.push(cc(SOSTENUTO_PEDAL, 0)).unwrap();
		for _ in 0..40 {
			voices.advance(TICK);
		}
		assert_eq!(at_a4(&voices), 0.0);
	}

	#[test]
	fn the_soft_pedal_does_nothing() {
		let (mut writer, mut voices) = voices_of(bank(six_db_a_tick()));
		writer.push(cc(67, 127)).unwrap();
		writer.push(on(69, 1.0)).unwrap();
		voices.advance(TICK);
		assert_eq!(db(at_a4(&voices)), -6.0);
	}

	#[test]
	fn volume_and_expression_scale_every_note_on_the_channel() {
		let (mut writer, reader) = RingBuffer::new(RING_CAPACITY);
		let mut voices = Voices::new(bank(six_db_a_tick()), reader);
		writer.push(on(69, 1.0)).unwrap();
		voices.advance(TICK);
		// The defaults, volume 100 and expression 127: 4.15 dB down.
		assert_eq!(db(at_a4(&voices)), -6.0 - 4.152149);
		writer.push(cc(EXPRESSION, 64)).unwrap();
		writer.push(cc(VOLUME, 127)).unwrap();
		voices.advance(0.0);
		let expression = (64.0f64 / 127.0).powi(2);
		assert!((at_a4(&voices) - db_to_amplitude(6.0) * expression).abs() < 1e-9);
	}

	#[test]
	fn pitch_bend_moves_a_note_by_its_range_and_rpn_zero_sets_the_range() {
		let (mut writer, mut voices) = voices_of(bank(Envelope {
			decay: 1e6,
			..Envelope::default()
		}));
		let bend = |value| NoteEvent::MidiPitchBend {
			timing: 0,
			channel: 0,
			value,
		};
		writer.push(on(69, 1.0)).unwrap();
		// A whole tone up at the default range of two semitones: A4 → B4.
		writer.push(bend(1.0)).unwrap();
		voices.advance(TICK);
		let values = drawn(&voices);
		assert!(values[14] > 0.9 && values[12] == 0.0, "{values:?}");

		// Range 24 semitones: the same wheel position is two octaves up, off the grid.
		for event in [cc(RPN_MSB, 0), cc(RPN_LSB, 0), cc(DATA_ENTRY_MSB, 24)] {
			writer.push(event).unwrap();
		}
		voices.advance(0.0);
		assert!(drawn(&voices).iter().all(|&v| v == 0.0));
		writer.push(bend(0.5)).unwrap();
		voices.advance(0.0);
		assert!(at_a4(&voices) > 0.9, "the wheel at rest");
	}
}
