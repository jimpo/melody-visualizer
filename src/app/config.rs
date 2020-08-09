use std::{
	collections::HashMap,
};

use crate::error::Error;
use crate::graphic::{
	GraphicGenerator,
	generators::spiral::{self, SpiralGenerator as Spiral},
};
use crate::source::SourceType;
use crate::spectrum::{
	Hz, SpectrumParams, SpectrumTransform,
	generators::audio,
	transforms::{diffuser::{self, Diffuser}, volume_normalizer::{self, VolumeNormalizer}},
};
use crate::traits::Configurable;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
	pub min_freq: Hz,
	pub max_freq: Hz,
	// Frequency domain samples per octave.
	pub samples_per_octave: usize,
	pub spectrum_generator: SpectrumGeneratorConfig,
	pub spectrum_transforms: HashMap<u64, SpectrumTransformConfig>,
	pub spectrum_transform_order: Vec<u64>,
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
			pub fn update(self, transform: &mut Box<dyn GraphicGenerator>) {
				match self {
					$(
						Self::$variant(config) => {
							match transform
								.upcast_any_mut()
								.downcast_mut::<$variant>()
							{
								Some(transform) => transform.set_config(config),
								None => *transform = Box::new($variant::new(config)),
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
			pub fn update(self, transform: &mut Box<dyn SpectrumTransform>) {
				match self {
					$(
						Self::$variant(config) => {
							match transform
								.upcast_any_mut()
								.downcast_mut::<$variant>()
							{
								Some(transform) => transform.set_config(config),
								None => *transform = Box::new($variant::new(config)),
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
		Diffuser,
		VolumeNormalizer,
	}
}

impl Default for Config {
	fn default() -> Self {
		Config {
			min_freq: 200.0, // Low-end of human hearing
			max_freq: 20000.0, // High-end of human hearing
			samples_per_octave: 180,
			spectrum_generator: SpectrumGeneratorConfig::Audio(audio::Config {
				dft_window_size: 2048,
			}),
			spectrum_transforms: vec![
				SpectrumTransformConfig::Diffuser(diffuser::Config {
					width: 1.0 / 24.0,
				}),
				SpectrumTransformConfig::VolumeNormalizer(volume_normalizer::Config {
					rate: 0.1,
				}),
			]
				.into_iter()
				.enumerate()
				.map(|(i, config)| (i as u64, config))
				.collect(),
			spectrum_transform_order: (0..2).into_iter().collect(),
			graphic_generator: GraphicGeneratorConfig::Spiral(spiral::Config {
				key_log_freq: 263.74, // C
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

	pub fn unused_transform_id(&self) -> u64 {
		let mut id = 0;
		while self.spectrum_transforms.contains_key(&id) {
			id += 1;
		}
		id
	}

	pub fn spectrum_transform_by_index(&self, index: usize)
		-> Result<Option<(u64, &SpectrumTransformConfig)>, Error>
	{
		if let Some(&id) = self.spectrum_transform_order.get(index) {
			let config = self.spectrum_transforms.get(&id)
				.ok_or_else(|| Error::MissingTransform { id })?;
			Ok(Some((id, config)))
		} else {
			Ok(None)
		}
	}
}