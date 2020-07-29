use crate::spectrum::{Hz, SpectrumParams};
use crate::audio_spectrum_generator;
use crate::spiral;
use crate::source::SourceType;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
	pub min_freq: Hz,
	pub max_freq: Hz,
	// Frequency domain samples per octave.
	pub samples_per_octave: usize,
	pub spectrum_generator: SpectrumGeneratorConfig,
	pub spectrum_transforms: Vec<SpectrumTransformConfig>,
	pub graphic_generator: GraphicGeneratorConfig,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpectrumGeneratorConfig {
	Audio(audio_spectrum_generator::Config),
}

impl SpectrumGeneratorConfig {
	pub fn source_type(&self) -> SourceType {
		match self {
			Self::Audio(_) => SourceType::Audio,
		}
	}
}

#[derive(Debug, Clone, PartialEq)]
pub enum GraphicGeneratorConfig {
	Spiral(spiral::Config),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SpectrumTransformConfig {
	VolumeNormalizer,
}

impl Default for Config {
	fn default() -> Self {
		Config {
			min_freq: 200.0f64.log2(), // Low-end of human hearing
			max_freq: 20000.0f64.log2(), // High-end of human hearing
			samples_per_octave: 180,
			spectrum_generator: SpectrumGeneratorConfig::Audio(audio_spectrum_generator::Config {
				dft_window_size: 2048,
			}),
			spectrum_transforms: vec![
				SpectrumTransformConfig::VolumeNormalizer,
			],
			graphic_generator: GraphicGeneratorConfig::Spiral(spiral::Config {
				key_log_freq: 263.74f64.log2(), // C
				outer_pad: 20.0,
				center_pad: 50.0,
			}),
		}
	}
}