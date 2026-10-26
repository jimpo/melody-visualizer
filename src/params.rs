//! The plugin's parameters: every control the GTK app's control pane had, as a
//! parameter the host can show, automate and save.
//!
//! [`PluginParams::config`] is the one place they are turned into the engine's
//! [`Config`]. The defaults are read back out of `Config::default()`, so the
//! two cannot drift apart.

use nice_plug::prelude::*;
use std::sync::Arc;

use crate::app::Config;
use crate::app::config::{
	GraphicGeneratorConfig, SpectrumGeneratorConfig, SpectrumTransformConfig,
};
use crate::note;
use crate::note::Note;
use crate::spectrum::generators::audio;

/// The ends of the pitch range, the 88 keys of a piano.
const LOWEST_NOTE: Note = note!(A, 0);
const HIGHEST_NOTE: Note = note!(C, 8);

/// The octave the key is set in. Only its pitch class shows: every octave of
/// the key sits on the same spoke of the spiral.
const KEY_OCTAVE: i8 = 4;

const SEMITONES_PER_OCTAVE: f64 = 12.0;
const CENTS_PER_OCTAVE: f64 = 1200.0;

/// The parameter IDs are saved in every host project that uses the plugin:
/// never change one.
#[derive(Params)]
pub struct PluginParams {
	#[id = "window_ms"]
	pub window_ms: IntParam,
	#[id = "update_rate"]
	pub update_rate: IntParam,

	#[id = "harmonic_summation"]
	pub harmonic_summation: BoolParam,
	#[id = "harmonics"]
	pub harmonics: IntParam,
	#[id = "harmonic_decay"]
	pub harmonic_decay: FloatParam,

	#[id = "power_map"]
	pub power_map: BoolParam,
	#[id = "exponent"]
	pub exponent: FloatParam,

	#[id = "diffuser"]
	pub diffuser: BoolParam,
	#[id = "diffuser_width"]
	pub diffuser_width: FloatParam,

	// The volume normalizer has no switch: it is the stage that scales the
	// spectrum into the range the spiral draws, and the spiral goes dark without
	// it.
	#[id = "normalizer_rate"]
	pub normalizer_rate: FloatParam,

	/// The pitch range's ends, in semitones above [`LOWEST_NOTE`].
	#[id = "lowest_note"]
	pub lowest_note: IntParam,
	#[id = "highest_note"]
	pub highest_note: IntParam,
	/// The key, in semitones above C.
	#[id = "key"]
	pub key: IntParam,
	#[id = "interval_ring"]
	pub interval_ring: BoolParam,
	#[id = "exposure"]
	pub exposure: FloatParam,
	#[id = "center_pad"]
	pub center_pad: IntParam,
	#[id = "outer_pad"]
	pub outer_pad: IntParam,
	#[id = "glow_core"]
	pub glow_core: FloatParam,
	#[id = "glow_falloff"]
	pub glow_falloff: FloatParam,
}

impl Default for PluginParams {
	fn default() -> Self {
		let config = Config::default();
		let SpectrumGeneratorConfig::Audio(analysis) = &config.spectrum_generator;
		let GraphicGeneratorConfig::Spiral(spiral) = &config.graphic_generator;
		// Each stage's parameters, and whether it runs, from the default chain.
		let mut harmonic_summation = None;
		let mut power_map = None;
		let mut diffuser = None;
		let mut normalizer = None;
		for entry in &config.spectrum_transforms {
			match &entry.config {
				SpectrumTransformConfig::HarmonicSummation(c) => {
					harmonic_summation = Some((entry.enabled, c.clone()))
				}
				SpectrumTransformConfig::PowerMap(c) => {
					power_map = Some((entry.enabled, c.clone()))
				}
				SpectrumTransformConfig::Diffuser(c) => diffuser = Some((entry.enabled, c.clone())),
				SpectrumTransformConfig::VolumeNormalizer(c) => normalizer = Some(c.clone()),
				SpectrumTransformConfig::DecibelConverter(_) => {}
			}
		}
		let missing = "the default chain holds every stage the parameters control";
		let harmonic_summation = harmonic_summation.expect(missing);
		let power_map = power_map.expect(missing);
		let diffuser = diffuser.expect(missing);
		let normalizer = normalizer.expect(missing);

		let notes = (HIGHEST_NOTE - LOWEST_NOTE) as i32;
		let key = Note::nearest(spiral.key_log_freq) - note!(C, KEY_OCTAVE);

		Self {
			window_ms: IntParam::new(
				"Window",
				analysis.window_ms as i32,
				linear(audio::MIN_WINDOW_MS as i32, audio::MAX_WINDOW_MS as i32),
			)
			.with_unit(" ms"),
			update_rate: IntParam::new(
				"Update rate",
				analysis.update_rate as i32,
				linear(audio::MIN_UPDATE_RATE as i32, audio::MAX_UPDATE_RATE as i32),
			)
			.with_unit(" / sec"),

			harmonic_summation: BoolParam::new("Harmonic summation", harmonic_summation.0),
			harmonics: IntParam::new(
				"Harmonics",
				harmonic_summation.1.harmonics as i32,
				linear(1, 16),
			)
			.with_unit(" harmonics"),
			harmonic_decay: FloatParam::new(
				"Harmonic decay",
				harmonic_summation.1.decay as f32,
				FloatRange::Linear { min: 0.0, max: 1.0 },
			)
			.with_step_size(0.01)
			.with_value_to_string(formatters::v2s_f32_rounded(2)),

			power_map: BoolParam::new("Power map", power_map.0),
			exponent: FloatParam::new(
				"Exponent",
				power_map.1.exponent as f32,
				FloatRange::Linear { min: 0.5, max: 4.0 },
			)
			.with_step_size(0.1)
			.with_value_to_string(formatters::v2s_f32_rounded(1)),

			diffuser: BoolParam::new("Diffuser", diffuser.0),
			diffuser_width: FloatParam::new(
				"Diffuser width",
				(diffuser.1.width * SEMITONES_PER_OCTAVE) as f32,
				FloatRange::Linear {
					min: 0.0,
					max: 10.0,
				},
			)
			.with_step_size(0.1)
			.with_value_to_string(formatters::v2s_f32_rounded(1))
			.with_unit(" semitones"),

			normalizer_rate: FloatParam::new(
				"Normalizer rate",
				normalizer.rate as f32,
				FloatRange::Linear {
					min: 0.01,
					max: 1.0,
				},
			)
			.with_step_size(0.01)
			.with_value_to_string(formatters::v2s_f32_rounded(2)),

			lowest_note: IntParam::new(
				"Lowest note",
				(Note::nearest(config.min_freq.log2()) - LOWEST_NOTE) as i32,
				linear(0, notes),
			)
			.with_value_to_string(Arc::new(note_name))
			.with_string_to_value(Arc::new(parse_name(notes, note_name))),
			highest_note: IntParam::new(
				"Highest note",
				(Note::nearest(config.max_freq.log2()) - LOWEST_NOTE) as i32,
				linear(0, notes),
			)
			.with_value_to_string(Arc::new(note_name))
			.with_string_to_value(Arc::new(parse_name(notes, note_name))),
			key: IntParam::new("Key", key as i32, linear(0, 11))
				.with_value_to_string(Arc::new(key_name))
				.with_string_to_value(Arc::new(parse_name(11, key_name))),
			interval_ring: BoolParam::new("Interval ring", spiral.interval_ring),
			exposure: FloatParam::new(
				"Exposure",
				spiral.exposure as f32,
				FloatRange::Linear { min: 0.5, max: 4.0 },
			)
			.with_step_size(0.05)
			.with_value_to_string(formatters::v2s_f32_rounded(2))
			.with_unit("×"),
			center_pad: IntParam::new("Centre hole", spiral.center_pad as i32, linear(0, 200))
				.with_unit(" px"),
			outer_pad: IntParam::new("Outer padding", spiral.outer_pad as i32, linear(0, 100))
				.with_unit(" px"),
			glow_core: FloatParam::new(
				"Glow core",
				(spiral.core * CENTS_PER_OCTAVE) as f32,
				FloatRange::Linear {
					min: 0.0,
					max: 120.0,
				},
			)
			.with_step_size(1.0)
			.with_value_to_string(formatters::v2s_f32_rounded(0))
			.with_unit(" ¢"),
			glow_falloff: FloatParam::new(
				"Glow falloff",
				(spiral.sigma * CENTS_PER_OCTAVE) as f32,
				FloatRange::Linear {
					min: 0.0,
					max: 90.0,
				},
			)
			.with_step_size(1.0)
			.with_value_to_string(formatters::v2s_f32_rounded(0))
			.with_unit(" ¢"),
		}
	}
}

impl PluginParams {
	/// The pitch range's ends, lowest first. A range whose ends have crossed
	/// is the same range read upwards.
	pub fn pitch_range(&self) -> (Note, Note) {
		let (low, high) = (self.lowest_note.value(), self.highest_note.value());
		let note = |semitones: i32| LOWEST_NOTE + semitones as isize;
		(note(low.min(high)), note(low.max(high)))
	}

	/// The engine configuration the parameters describe right now.
	pub fn config(&self) -> Config {
		let mut config = Config::default();

		let (lowest, highest) = self.pitch_range();
		config.min_freq = lowest.frequency();
		config.max_freq = highest.frequency();

		config.spectrum_generator = SpectrumGeneratorConfig::Audio(audio::Config {
			window_ms: self.window_ms.value() as u32,
			update_rate: self.update_rate.value() as u32,
		});

		for entry in &mut config.spectrum_transforms {
			match &mut entry.config {
				SpectrumTransformConfig::HarmonicSummation(c) => {
					entry.enabled = self.harmonic_summation.value();
					c.harmonics = self.harmonics.value() as usize;
					c.decay = plain(self.harmonic_decay.value());
				}
				SpectrumTransformConfig::PowerMap(c) => {
					entry.enabled = self.power_map.value();
					c.exponent = plain(self.exponent.value());
				}
				SpectrumTransformConfig::Diffuser(c) => {
					entry.enabled = self.diffuser.value();
					c.width = plain(self.diffuser_width.value()) / SEMITONES_PER_OCTAVE;
				}
				SpectrumTransformConfig::VolumeNormalizer(c) => {
					c.rate = plain(self.normalizer_rate.value());
				}
				SpectrumTransformConfig::DecibelConverter(_) => {}
			}
		}

		let GraphicGeneratorConfig::Spiral(spiral) = &mut config.graphic_generator;
		spiral.key_log_freq = (note!(C, KEY_OCTAVE) + self.key.value() as isize).log_frequency();
		spiral.interval_ring = self.interval_ring.value();
		spiral.exposure = plain(self.exposure.value());
		spiral.center_pad = self.center_pad.value().into();
		spiral.outer_pad = self.outer_pad.value().into();
		spiral.core = plain(self.glow_core.value()) / CENTS_PER_OCTAVE;
		spiral.sigma = plain(self.glow_falloff.value()) / CENTS_PER_OCTAVE;

		config
	}
}

/// A note of the pitch range, by its semitones above [`LOWEST_NOTE`].
fn note_name(semitones: i32) -> String {
	(LOWEST_NOTE + semitones as isize).to_string()
}

/// Each key's name, by its semitones above C, and whether it is a black key.
pub const KEYS: [(&str, bool); 12] = [
	("C", false),
	("C♯", true),
	("D", false),
	("E♭", true),
	("E", false),
	("F", false),
	("F♯", true),
	("G", false),
	("A♭", true),
	("A", false),
	("B♭", true),
	("B", false),
];

/// A key, by its semitones above C.
fn key_name(semitones: i32) -> String {
	KEYS[semitones.rem_euclid(12) as usize].0.to_string()
}

/// The inverse of `name` over `0..=max`: the value whose name a host hands
/// back, as the user typed it or as `name` wrote it.
fn parse_name(max: i32, name: fn(i32) -> String) -> impl Fn(&str) -> Option<i32> + Send + Sync {
	move |text| (0..=max).find(|&value| name(value) == text.trim())
}

fn linear(min: i32, max: i32) -> IntRange {
	IntRange::Linear { min, max }
}

/// A parameter's value as the decimal it was set to.
///
/// The parameters hold `f32`, so a 0.8 comes back as 0.800000011920929 in
/// `f64`. Every control moves in steps far coarser than a millionth, so rounding
/// to six places recovers the value exactly.
fn plain(value: f32) -> f64 {
	(f64::from(value) * 1e6).round() / 1e6
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn the_default_parameters_describe_the_default_config() {
		assert_eq!(PluginParams::default().config(), Config::default());
	}
}
