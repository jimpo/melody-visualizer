//! The SoundFont the MIDI generator reads its voices from.
//!
//! The generator draws one partial per voice and never plays a sample, so it
//! keeps from the font only what moves that partial's loudness, pitch or
//! lifetime: for each preset, its zones with their key and velocity ranges,
//! volume envelope, attenuation and tuning. This module is the only user of
//! the `rustysynth` crate, which parses the file; the samples it reads are
//! dropped with the parsed font once the zones are built.
//!
//! Units follow the SoundFont 2.01 spec, section 8.1.3.

use std::{fmt, fs::File, io::BufReader, ops::RangeInclusive, path::Path};

use rustysynth::{InstrumentRegion, PresetRegion, SoundFont};

use super::midi::Envelope;

/// The stripped SoundFont the generator falls back to: the piano family of
/// GeneralUser GS with its samples replaced by silence (see
/// `assets/soundfont/README.md`).
const DEFAULT_FONT: &[u8] = include_bytes!("../../../assets/soundfont/piano.sf2");

/// The longest a SoundFont's sustain level is: 100 dB down is silence.
const MAX_SUSTAIN_DB: f64 = 100.0;

/// Why a SoundFont could not be loaded.
#[derive(Debug, derive_more::Display, derive_more::From, derive_more::Error)]
pub enum LoadError {
	#[display("reading the file: {_0}")]
	Io(std::io::Error),
	#[display("parsing the SoundFont: {_0}")]
	#[from(skip)]
	Parse(#[error(not(source))] String),
	#[display("the SoundFont has no presets")]
	#[from(skip)]
	NoPresets,
}

/// One instrument zone that plays inside one preset zone, its settings merged
/// as a SoundFont synth merges them.
#[derive(Debug, Clone, PartialEq)]
pub struct Zone {
	keys: RangeInclusive<u8>,
	velocities: RangeInclusive<u8>,
	pub envelope: Envelope,
	/// The zone's own attenuation, in dB, before velocity and the controllers.
	pub attenuation: f64,
	/// The key the sample was recorded at, as a MIDI key number.
	root_key: f64,
	/// Cents added to the pitch: coarse and fine tune, and the sample's pitch
	/// correction.
	tune: f64,
	/// Cents of pitch per key.
	scale_tuning: f64,
}

impl Zone {
	/// A zone that plays every key and velocity at its natural pitch.
	pub fn new(envelope: Envelope) -> Self {
		Zone {
			keys: 0..=127,
			velocities: 0..=127,
			envelope,
			attenuation: 0.0,
			root_key: 60.0,
			tune: 0.0,
			scale_tuning: 100.0,
		}
	}

	/// Whether the zone sounds for `key` struck at MIDI velocity `velocity`.
	pub fn plays(&self, key: u8, velocity: u8) -> bool {
		self.keys.contains(&key) && self.velocities.contains(&velocity)
	}

	/// The pitch of the fundamental `key` plays, in cents above MIDI key 0:
	/// 100 a key at the default scale tuning, so A4 is 6900.
	pub fn cents(&self, key: u8) -> f64 {
		self.root_key * 100.0 + (f64::from(key) - self.root_key) * self.scale_tuning + self.tune
	}
}

/// A preset: the zones that play for a key and velocity.
#[derive(Debug, Clone, PartialEq)]
struct Preset {
	bank: u8,
	patch: u8,
	zones: Vec<Zone>,
}

/// The presets of a SoundFont, reduced to the zones the generator reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Bank {
	presets: Vec<Preset>,
}

impl Bank {
	/// A bank whose only preset, bank 0 patch 0, has `zones`.
	pub fn from_zones(zones: Vec<Zone>) -> Self {
		Bank {
			presets: vec![Preset {
				bank: 0,
				patch: 0,
				zones,
			}],
		}
	}

	/// The SoundFont file at `path`.
	///
	/// Reads and parses the whole file, samples included: call it off the
	/// audio thread.
	pub fn load(path: &Path) -> Result<Self, LoadError> {
		Self::parse(&mut BufReader::new(File::open(path)?))
	}

	/// The SoundFont committed to the repository.
	pub fn fallback() -> Self {
		Self::parse(&mut &*DEFAULT_FONT).expect("the committed SoundFont parses")
	}

	/// The file at `path` if it is set and loads, otherwise the fallback.
	/// A file that fails is logged.
	pub fn load_or_fallback(path: Option<&Path>) -> Self {
		path.and_then(|path| match Self::load(path) {
			Ok(bank) => Some(bank),
			Err(error) => {
				log::warn!("SoundFont {}: {error}; using the default", path.display());
				None
			}
		})
		.unwrap_or_else(Self::fallback)
	}

	fn parse(reader: &mut impl std::io::Read) -> Result<Self, LoadError> {
		let font = SoundFont::new(reader).map_err(|e| LoadError::Parse(e.to_string()))?;
		let instruments = font.get_instruments();
		let presets = font
			.get_presets()
			.iter()
			.map(|preset| {
				let zones = preset
					.get_regions()
					.iter()
					.flat_map(|outer| {
						instruments[outer.get_instrument_id()]
							.get_regions()
							.iter()
							.map(move |inner| (outer, inner))
					})
					.map(|(outer, inner)| merge(outer, inner))
					.collect();
				Preset {
					bank: preset.get_bank_number().clamp(0, 127) as u8,
					patch: preset.get_patch_number().clamp(0, 127) as u8,
					zones,
				}
			})
			.collect::<Vec<_>>();
		if presets.is_empty() {
			return Err(LoadError::NoPresets);
		}
		Ok(Bank { presets })
	}

	/// The zones of the preset at `bank` and `patch` that sound for `key` at
	/// `velocity` (0..=127): the instrument zones whose ranges hold the note,
	/// inside preset zones whose ranges do too. A bank without that preset
	/// falls back to its first.
	pub fn zones(&self, bank: u8, patch: u8, key: u8, velocity: u8) -> impl Iterator<Item = &Zone> {
		let preset = self
			.presets
			.iter()
			.find(|preset| preset.bank == bank && preset.patch == patch)
			.unwrap_or(&self.presets[0]);
		preset
			.zones
			.iter()
			.filter(move |zone| zone.plays(key, velocity))
	}
}

impl fmt::Display for Bank {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		write!(f, "{} presets", self.presets.len())
	}
}

/// An instrument zone and the preset zone it plays inside, as one [`Zone`].
///
/// Each side already has its layer's global zone folded in by the parser. The
/// preset's value adds to the instrument's: times are multiplied (the preset's
/// getters give the factor `2^(offset/1200)`), levels and tunings added.
fn merge(outer: &PresetRegion, inner: &InstrumentRegion) -> Zone {
	let keys = intersect(
		(outer.get_key_range_start(), outer.get_key_range_end()),
		(inner.get_key_range_start(), inner.get_key_range_end()),
	);
	let velocities = intersect(
		(
			outer.get_velocity_range_start(),
			outer.get_velocity_range_end(),
		),
		(
			inner.get_velocity_range_start(),
			inner.get_velocity_range_end(),
		),
	);
	let envelope = Envelope {
		delay: f64::from(inner.get_delay_volume_envelope() * outer.get_delay_volume_envelope()),
		attack: f64::from(inner.get_attack_volume_envelope() * outer.get_attack_volume_envelope()),
		hold: f64::from(inner.get_hold_volume_envelope() * outer.get_hold_volume_envelope()),
		decay: f64::from(inner.get_decay_volume_envelope() * outer.get_decay_volume_envelope()),
		sustain: (f64::from(inner.get_sustain_volume_envelope())
			+ f64::from(outer.get_sustain_volume_envelope()))
		.clamp(0.0, MAX_SUSTAIN_DB),
		release: f64::from(
			inner.get_release_volume_envelope() * outer.get_release_volume_envelope(),
		),
		key_to_hold: f64::from(
			inner.get_key_number_to_volume_envelope_hold()
				+ outer.get_key_number_to_volume_envelope_hold(),
		),
		key_to_decay: f64::from(
			inner.get_key_number_to_volume_envelope_decay()
				+ outer.get_key_number_to_volume_envelope_decay(),
		),
	};
	Zone {
		keys: keys.0..=keys.1,
		velocities: velocities.0..=velocities.1,
		envelope,
		attenuation: f64::from(inner.get_initial_attenuation() + outer.get_initial_attenuation()),
		root_key: f64::from(inner.get_root_key()),
		tune: f64::from(
			100 * (inner.get_coarse_tune() + outer.get_coarse_tune())
				+ inner.get_fine_tune()
				+ outer.get_fine_tune(),
		),
		scale_tuning: f64::from(inner.get_scale_tuning() + outer.get_scale_tuning()),
	}
}

/// The overlap of two inclusive ranges, empty (start above end) if they miss.
fn intersect(a: (i32, i32), b: (i32, i32)) -> (u8, u8) {
	let clamp = |n: i32| n.clamp(0, 127) as u8;
	let (low, high) = (clamp(a.0.max(b.0)), clamp(a.1.min(b.1)));
	if a.0.max(b.0) > a.1.min(b.1) {
		(1, 0)
	} else {
		(low, high)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_default_font_is_a_piano_with_a_few_zones_a_key() {
		let bank = Bank::fallback();
		for key in 21..=108 {
			let count = bank.zones(0, 0, key, 100).count();
			assert!((1..=4).contains(&count), "key {key} plays {count} zones");
		}
		assert_eq!(
			bank.zones(0, 0, 10, 100).count(),
			bank.zones(0, 0, 10, 100).count()
		);
	}

	#[test]
	fn an_empty_range_never_plays() {
		assert_eq!(intersect((0, 40), (50, 127)), (1, 0));
		assert_eq!(intersect((0, 60), (50, 127)), (50, 60));
	}

	#[test]
	fn a_zone_plays_the_pitch_its_tuning_gives() {
		let mut zone = Zone::new(Envelope::default());
		zone.root_key = 60.0;
		zone.tune = 10.0;
		zone.scale_tuning = 101.0;
		// One key above the root: 101 cents on top of 6000, plus the tuning.
		assert_eq!(zone.cents(61), 6000.0 + 101.0 + 10.0);
	}

	#[test]
	fn a_bad_file_is_an_error_and_falls_back() {
		let missing = Path::new("/nonexistent/piano.sf2");
		assert!(Bank::load(missing).is_err());
		assert_eq!(Bank::load_or_fallback(Some(missing)), Bank::fallback());
		assert_eq!(Bank::load_or_fallback(None), Bank::fallback());
	}
}
