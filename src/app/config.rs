use std::any::Any;

use crate::error::Error;
use crate::graphic::{
	GraphicGenerator,
	generators::spiral::{self, SpiralGenerator as Spiral},
};
use crate::note;
use crate::spectrum::{
	Hz, SpectrumGenerator, SpectrumParams, SpectrumTransform, TransformChain, TransformId,
	generators::{SourceGenerator, audio, midi},
	transforms::{
		decibel_converter::DecibelConverter,
		diffuser::{self, Diffuser},
		harmonic_summation::{self, HarmonicSummation},
		power_map::{self, PowerMap},
		volume_normalizer::{self, VolumeNormalizer},
	},
};
use crate::traits::Configurable;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Config {
	pub min_freq: Hz,
	pub max_freq: Hz,
	// Frequency domain samples per octave.
	pub samples_per_octave: usize,
	pub spectrum_generator: SpectrumGeneratorConfig,
	/// The transform chain, in the order it is applied. Mirrors the shape of
	/// [`TransformChain`](crate::spectrum::TransformChain), so the two cannot
	/// disagree about order, membership or which stages run.
	pub spectrum_transforms: Vec<TransformEntry>,
	pub graphic_generator: GraphicGeneratorConfig,
}

/// One stage of the transform chain: the id it is addressed by, how it is
/// configured, and whether it runs.
///
/// `enabled` is the bypass switch's state. It lives here so that rebuilding the
/// chain from the config keeps a bypassed stage bypassed.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TransformEntry {
	pub id: TransformId,
	pub config: SpectrumTransformConfig,
	pub enabled: bool,
}

/// Builds the chain the entries describe, in the order they are given.
impl FromIterator<TransformEntry> for TransformChain {
	fn from_iter<Entries: IntoIterator<Item = TransformEntry>>(entries: Entries) -> Self {
		let mut chain = TransformChain::default();
		for (index, entry) in entries.into_iter().enumerate() {
			chain.insert(index, entry.id, entry.config.create());
			chain.set_enabled(entry.id, entry.enabled);
		}
		chain
	}
}

/// The generator's settings: the audio analysis it runs until the host plays a
/// note, and the envelope it gives notes from then on.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpectrumGeneratorConfig {
	pub audio: audio::Config,
	pub envelope: midi::Envelope,
}

impl SpectrumGeneratorConfig {
	/// Reconfigures `generator` in place.
	///
	/// The generator holds the inputs it reads from, so it is reconfigured
	/// rather than rebuilt: rebuilding one would mean opening them again.
	pub fn update(self, generator: &mut dyn SpectrumGenerator) -> Result<(), Error> {
		match (generator as &mut dyn Any).downcast_mut::<SourceGenerator>() {
			Some(generator) => {
				generator.set_config(self);
				Ok(())
			}
			None => Err(Error::UnexpectedConfigEntry(
				"config names the source generator, but the chain holds another kind".to_string(),
			)),
		}
	}
}

macro_rules! define_graphic_generator_config {
	($(#[$attr:meta])* $vis:vis enum $name:ident { $($variant:ident,)+ }) => {
		$(#[$attr])*
		$vis enum $name {
			$(
				$variant(<$variant as Configurable>::Config),
			)+
		}

		impl $name {
			pub fn create(self) -> Box<dyn GraphicGenerator> {
				match self {
					$(
						Self::$variant(config) => Box::new($variant::new(config)),
					)+
				}
			}

			/// Reconfigures `generator` in place, or replaces it when the
			/// config names a different kind.
			///
			/// Replacing is why this takes the `Box` rather than
			/// `&mut dyn GraphicGenerator`. Reconfiguring in place is what
			/// keeps a slider drag from rebuilding the generator per event.
			pub fn update(self, generator: &mut Box<dyn GraphicGenerator>) {
				match self {
					$(
						Self::$variant(config) => {
							match generator
								.as_any_mut()
								.downcast_mut::<$variant>()
							{
								Some(generator) => generator.set_config(config),
								None => *generator = Box::new($variant::new(config)),
							}
						}
					)+
				}
			}
		}
	};
}

define_graphic_generator_config! {
	#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
	pub enum GraphicGeneratorConfig {
		Spiral,
	}
}

macro_rules! define_spectrum_transform_config {
	($(#[$attr:meta])* $vis:vis enum $name:ident { $($variant:ident,)+ }) => {
		$(#[$attr])*
		$vis enum $name {
			$(
				$variant(<$variant as Configurable>::Config),
			)+
		}

		impl $name {
			pub fn create(self) -> Box<dyn SpectrumTransform> {
				match self {
					$(
						Self::$variant(config) => Box::new($variant::new(config)),
					)+
				}
			}

			pub fn update(self, transform: &mut dyn SpectrumTransform) -> Result<(), Error> {
				match self {
					$(
						Self::$variant(config) => {
							match transform
								.as_any_mut()
								.downcast_mut::<$variant>()
							{
								Some(transform) => {
									transform.set_config(config);
									Ok(())
								}
								None => Err(Error::UnexpectedConfigEntry(format!(
									"config names a {}, but the chain holds another transform",
									stringify!($variant),
								))),
							}
						}
					)+
				}
			}
		}
	};
}

define_spectrum_transform_config! {
	 #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
	pub enum SpectrumTransformConfig {
		DecibelConverter,
		Diffuser,
		HarmonicSummation,
		PowerMap,
		VolumeNormalizer,
	}
}

impl Default for Config {
	fn default() -> Self {
		let transforms = vec![
			// SpectrumTransformConfig::DecibelConverter(decibel_converter::Config {
			// 	min_level: 1.0e-3,
			// }),
			SpectrumTransformConfig::HarmonicSummation(harmonic_summation::Config {
				decay: 0.8,
				harmonics: 8,
			}),
			SpectrumTransformConfig::PowerMap(power_map::Config { exponent: 2.0 }),
			SpectrumTransformConfig::Diffuser(diffuser::Config { width: 1.0 / 24.0 }),
			SpectrumTransformConfig::VolumeNormalizer(volume_normalizer::Config { rate: 0.1 }),
		];
		Config {
			min_freq: note!(D, 2).frequency(),
			max_freq: note!(D, 6).frequency(),
			// A fifteenth of a semitone. The density of the display grid, which
			// no control moves.
			samples_per_octave: 180,
			spectrum_generator: SpectrumGeneratorConfig {
				audio: audio::Config::default(),
				envelope: midi::Envelope::default(),
			},
			spectrum_transforms: transforms
				.into_iter()
				.enumerate()
				.map(|(index, config)| TransformEntry {
					id: TransformId(index as u64),
					config,
					enabled: true,
				})
				.collect(),
			graphic_generator: GraphicGeneratorConfig::Spiral(spiral::Config {
				key_log_freq: note!(C, 4).log_frequency(),
				outer_pad: 20.0,
				center_pad: 50.0,
				interval_ring: false,
				exposure: spiral::DEFAULT_EXPOSURE,
				core: spiral::DEFAULT_CORE,
				sigma: spiral::DEFAULT_SIGMA,
			}),
		}
	}
}

impl Config {
	pub fn spectrum_params(&self) -> SpectrumParams {
		let octaves = self.max_freq.log2() - self.min_freq.log2();
		let samples = (self.samples_per_octave as f64 * octaves).round() as usize;
		// there must be at least two samples, one at min_freq and one at max_freq
		let samples = samples.max(2);
		SpectrumParams::exp_spaced(samples, self.min_freq, self.max_freq)
	}

	/// Switches off the stages that make no sense for notes from MIDI.
	///
	/// A MIDI note is drawn without overtones, so harmonic summation would only
	/// add one note to another an octave or a fifth below it.
	pub fn for_midi(&mut self) {
		for entry in &mut self.spectrum_transforms {
			if let SpectrumTransformConfig::HarmonicSummation(_) = entry.config {
				entry.enabled = false;
			}
		}
	}

	pub fn spectrum_transform(&self, id: TransformId) -> Result<&SpectrumTransformConfig, Error> {
		self.spectrum_transform_entry(id).map(|entry| &entry.config)
	}

	pub fn spectrum_transform_mut(
		&mut self,
		id: TransformId,
	) -> Result<&mut SpectrumTransformConfig, Error> {
		self.spectrum_transform_entry_mut(id)
			.map(|entry| &mut entry.config)
	}

	fn spectrum_transform_entry(&self, id: TransformId) -> Result<&TransformEntry, Error> {
		self.spectrum_transforms
			.iter()
			.find(|entry| entry.id == id)
			.ok_or(Error::MissingTransform { id })
	}

	/// Runs or bypasses the transform identified by `id`.
	///
	/// Only the flag is reachable: an entry's id is what the chain, the config
	/// and the controls address it by, so nothing outside may change it.
	pub fn set_transform_enabled(&mut self, id: TransformId, enabled: bool) -> Result<(), Error> {
		self.spectrum_transform_entry_mut(id)?.enabled = enabled;
		Ok(())
	}

	fn spectrum_transform_entry_mut(
		&mut self,
		id: TransformId,
	) -> Result<&mut TransformEntry, Error> {
		self.spectrum_transforms
			.iter_mut()
			.find(|entry| entry.id == id)
			.ok_or(Error::MissingTransform { id })
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::note::Note;
	use crate::test_support::sample_reader;

	#[test]
	fn a_config_update_reaches_a_running_generator() {
		let config = Config::default().spectrum_generator;
		let (_notes, reader) = rtrb::RingBuffer::new(1);
		let mut generator = SourceGenerator::new(
			config.clone(),
			sample_reader(&[]),
			reader,
			48_000,
			Default::default(),
		);
		let before = generator.interval();

		SpectrumGeneratorConfig {
			audio: audio::Config {
				update_rate: config.audio.update_rate * 2,
				..config.audio
			},
			..config
		}
		.update(&mut generator)
		.expect("the generator is of the kind the config names");

		let ticks = generator.interval().as_secs_f64() * 2.0;
		assert!(
			(ticks - before.as_secs_f64()).abs() < 1e-6,
			"the generator ticks every {} s, expected {} s",
			generator.interval().as_secs_f64(),
			before.as_secs_f64() / 2.0,
		);
	}

	#[test]
	fn the_spiral_key_is_a_log_frequency_of_a_note_on_the_keyboard() {
		let GraphicGeneratorConfig::Spiral(config) = Config::default().graphic_generator;
		// The control pane's key row presses the key of the note
		// nearest `key_log_freq`. A Hz value in the field names a note some
		// thirty octaves up, whose pitch class has nothing to do with the key.
		let range = note!(A, 0)..=note!(C, 8);
		let key = Note::nearest(config.key_log_freq);
		assert!(
			range.contains(&key),
			"key_log_freq {} is {key}, outside the {:?} on a keyboard",
			config.key_log_freq,
			range,
		);
	}
}
