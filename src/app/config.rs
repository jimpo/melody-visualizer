use std::any::Any;

use crate::audio::source::SourceType;
use crate::error::Error;
use crate::graphic::{
	GraphicGenerator,
	generators::spiral::{self, SpiralGenerator as Spiral},
};
use crate::note;
use crate::spectrum::{
	Hz, SpectrumGenerator, SpectrumParams, SpectrumTransform, TransformId,
	generators::audio::{self, AudioSpectrumGenerator},
	transforms::{
		decibel_converter::DecibelConverter,
		diffuser::{self, Diffuser},
		volume_normalizer::{self, VolumeNormalizer},
	},
};
use crate::traits::Configurable;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
	pub min_freq: Hz,
	pub max_freq: Hz,
	// Frequency domain samples per octave.
	pub samples_per_octave: usize,
	pub spectrum_generator: SpectrumGeneratorConfig,
	/// The transform chain, in the order it is applied. Mirrors the shape of
	/// [`TransformChain`](crate::spectrum::TransformChain), so the two cannot
	/// disagree about order or membership.
	pub spectrum_transforms: Vec<(TransformId, SpectrumTransformConfig)>,
	pub graphic_generator: GraphicGeneratorConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpectrumGeneratorConfig {
	Audio(audio::Config),
}

impl SpectrumGeneratorConfig {
	pub fn source_type(&self) -> SourceType {
		match self {
			Self::Audio(_) => SourceType::Audio,
		}
	}

	/// Reconfigures `generator` in place.
	///
	/// The generator holds the audio input it reads from, so it is
	/// reconfigured rather than rebuilt: rebuilding one would mean opening the
	/// source again.
	pub fn update(self, generator: &mut dyn SpectrumGenerator) -> Result<(), Error> {
		match self {
			Self::Audio(config) => {
				match (generator as &mut dyn Any).downcast_mut::<AudioSpectrumGenerator>() {
					Some(generator) => {
						generator.set_config(config);
						Ok(())
					}
					None => Err(Error::UnexpectedConfigEntry(
						"config names an audio generator, but the chain holds another kind"
							.to_string(),
					)),
				}
			}
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
	#[derive(Debug, Clone, PartialEq)]
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
	 #[derive(Debug, Clone, PartialEq)]
	pub enum SpectrumTransformConfig {
		DecibelConverter,
		Diffuser,
		VolumeNormalizer,
	}
}

impl Default for Config {
	fn default() -> Self {
		let transforms = vec![
			// SpectrumTransformConfig::DecibelConverter(decibel_converter::Config {
			// 	min_level: 1.0e-6,
			// }),
			SpectrumTransformConfig::Diffuser(diffuser::Config { width: 1.0 / 24.0 }),
			SpectrumTransformConfig::VolumeNormalizer(volume_normalizer::Config { rate: 0.1 }),
		];
		Config {
			min_freq: 200.0,   // Low-end of human hearing
			max_freq: 20000.0, // High-end of human hearing
			samples_per_octave: 180,
			spectrum_generator: SpectrumGeneratorConfig::Audio(audio::Config {
				dft_window_size: 2048,
				overlap: 0.5,
			}),
			spectrum_transforms: transforms
				.into_iter()
				.enumerate()
				.map(|(index, config)| (TransformId(index as u64), config))
				.collect(),
			graphic_generator: GraphicGeneratorConfig::Spiral(spiral::Config {
				key_log_freq: note!(C, 4).log_frequency(),
				outer_pad: 20.0,
				center_pad: 50.0,
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

	pub fn spectrum_transform(&self, id: TransformId) -> Result<&SpectrumTransformConfig, Error> {
		self.spectrum_transforms
			.iter()
			.find(|(entry_id, _config)| *entry_id == id)
			.map(|(_id, config)| config)
			.ok_or(Error::MissingTransform { id })
	}

	pub fn spectrum_transform_mut(
		&mut self,
		id: TransformId,
	) -> Result<&mut SpectrumTransformConfig, Error> {
		self.spectrum_transforms
			.iter_mut()
			.find(|(entry_id, _config)| *entry_id == id)
			.map(|(_id, config)| config)
			.ok_or(Error::MissingTransform { id })
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::note::Note;
	use crate::spectrum::generators::audio::AudioSpectrumGenerator;
	use crate::test_support::sample_reader;

	#[test]
	fn a_config_update_reaches_a_running_generator() {
		let config = audio::Config {
			dft_window_size: 2048,
			overlap: 0.5,
		};
		let mut generator = AudioSpectrumGenerator::new(config.clone(), sample_reader(&[]), 48_000);
		let half_overlapped = generator.interval();

		SpectrumGeneratorConfig::Audio(audio::Config {
			overlap: 0.75,
			..config
		})
		.update(&mut generator)
		.expect("the generator is of the kind the config names");

		// Three quarters of a window overlapping leaves half the hop that half
		// of one does, so the generator ticks twice as often.
		let ticks = generator.interval().as_secs_f64() * 2.0;
		assert!(
			(ticks - half_overlapped.as_secs_f64()).abs() < 1e-6,
			"the generator ticks every {} s, expected {} s",
			generator.interval().as_secs_f64(),
			half_overlapped.as_secs_f64() / 2.0,
		);
	}

	#[test]
	fn the_spiral_key_is_a_log_frequency_of_a_note_on_the_keyboard() {
		let GraphicGeneratorConfig::Spiral(config) = Config::default().graphic_generator;
		// The key row in `gui/control_pane.rs` presses the key of the note
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
