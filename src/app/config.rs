use std::{
	any::Any,
	fs,
	io::ErrorKind,
	path::{Path, PathBuf},
};

use crate::audio::source::SourceType;
use crate::error::Error;
use crate::graphic::{
	GraphicGenerator,
	generators::spiral::{self, SpiralGenerator as Spiral},
};
use crate::note;
use crate::spectrum::{
	Hz, SpectrumGenerator, SpectrumParams, SpectrumTransform, TransformChain, TransformId,
	generators::audio::{self, AudioSpectrumGenerator},
	transforms::{
		decibel_converter::DecibelConverter,
		diffuser::{self, Diffuser},
		volume_normalizer::{self, VolumeNormalizer},
	},
};
use crate::traits::Configurable;

const STATE_DIR_NAME: &str = "melody-visualizer";
const STATE_FILE_NAME: &str = "config.toml";
const TEMP_FILE_NAME: &str = "config.toml.tmp";

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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
		// A fifteenth of a semitone, which the "Pitch resolution" control in
		// `gui/controls/spectrum.rs` moves and the window follows.
		let samples_per_octave = 180;
		Config {
			min_freq: 200.0,   // Low-end of human hearing
			max_freq: 20000.0, // High-end of human hearing
			samples_per_octave,
			spectrum_generator: SpectrumGeneratorConfig::Audio(audio::Config {
				dft_window_size: audio::dft_window_size(samples_per_octave),
				overlap: 0.5,
			}),
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
			}),
		}
	}
}

impl Config {
	/// Reads the persisted configuration, or returns the defaults before the
	/// application has written one.
	pub fn load() -> Result<Self, Error> {
		Self::load_from(&state_file_path())
	}

	/// Atomically replaces the persisted configuration with this complete
	/// configuration tree.
	pub fn save(&self) -> Result<(), Error> {
		self.save_to(&state_file_path())
	}

	fn load_from(path: &Path) -> Result<Self, Error> {
		let serialized = match fs::read_to_string(path) {
			Ok(serialized) => serialized,
			Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Self::default()),
			Err(err) => return Err(Error::ConfigIo(err)),
		};
		toml::from_str(&serialized).map_err(Error::ConfigParse)
	}

	fn save_to(&self, path: &Path) -> Result<(), Error> {
		let parent = path
			.parent()
			.expect("the state file always has an application directory");
		fs::create_dir_all(parent).map_err(Error::ConfigIo)?;
		let serialized = toml::to_string_pretty(self).map_err(Error::ConfigSerialize)?;
		let temporary = parent.join(TEMP_FILE_NAME);
		fs::write(&temporary, serialized).map_err(Error::ConfigIo)?;
		fs::rename(temporary, path).map_err(Error::ConfigIo)
	}

	pub fn spectrum_params(&self) -> SpectrumParams {
		let octaves = self.max_freq.log2() - self.min_freq.log2();
		let samples = (self.samples_per_octave as f64 * octaves).round() as usize;
		// there must be at least two samples, one at min_freq and one at max_freq
		let samples = samples.max(2);
		SpectrumParams::exp_spaced(samples, self.min_freq, self.max_freq)
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

fn state_file_path() -> PathBuf {
	glib::user_state_dir()
		.join(STATE_DIR_NAME)
		.join(STATE_FILE_NAME)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::note::Note;
	use crate::spectrum::generators::audio::AudioSpectrumGenerator;
	use crate::test_support::sample_reader;
	use tempfile::tempdir;

	#[test]
	fn config_round_trips_through_toml() {
		let directory = tempdir().unwrap();
		let path = directory.path().join("state/config.toml");
		let mut config = Config {
			min_freq: 55.0,
			samples_per_octave: 96,
			..Config::default()
		};
		config.spectrum_transforms[0].enabled = false;

		config.save_to(&path).unwrap();

		assert_eq!(Config::load_from(&path).unwrap(), config);
		assert!(!path.with_file_name(TEMP_FILE_NAME).exists());
	}

	#[test]
	fn a_missing_state_file_loads_the_defaults() {
		let directory = tempdir().unwrap();

		assert_eq!(
			Config::load_from(&directory.path().join("missing.toml")).unwrap(),
			Config::default(),
		);
	}

	#[test]
	fn malformed_state_is_reported() {
		let directory = tempdir().unwrap();
		let path = directory.path().join("config.toml");
		fs::write(&path, "min_freq = [not a number]").unwrap();

		assert!(matches!(
			Config::load_from(&path),
			Err(Error::ConfigParse(_))
		));
	}

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
