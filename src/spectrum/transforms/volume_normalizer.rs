use std::{any::Any, cmp::Ordering};

use crate::spectrum::{Spectrum, SpectrumTransform};
use crate::traits::Configurable;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
	pub rate: f64,
}

#[derive(Debug)]
pub struct VolumeNormalizer {
	config: Config,
	max_value: f64,
}

impl Configurable for VolumeNormalizer {
	type Config = Config;

	fn new(config: Config) -> Self {
		VolumeNormalizer {
			config,
			max_value: 0.0,
		}
	}

	fn set_config(&mut self, config: Config) {
		self.config = config;
	}
}

impl SpectrumTransform for VolumeNormalizer {
	fn transform(&mut self, mut spectrum: Spectrum) -> Spectrum {
		if spectrum.params().samples() == 0 {
			return spectrum;
		}

		let spectrum_max = spectrum_max_value(&mut spectrum);
		let rate = self.config.rate;
		self.max_value = (1.0 - rate) * self.max_value + rate * spectrum_max;

		if spectrum_max != 0.0 {
			for value in spectrum.values_mut().iter_mut() {
				*value /= self.max_value;
			}
		}
		spectrum
	}

	fn upcast_any_ref(&self) -> &dyn Any {
		self
	}

	fn upcast_any_mut(&mut self) -> &mut dyn Any {
		self
	}
}

fn spectrum_min_value(spectrum: &mut Spectrum) -> f64 {
	// f64 does not impl Ord because of NaN's and other weird edge cases.
	// We also depend here on the fact that spectrum values must be positive.
	spectrum.values().iter().fold(0.0, |min, val| {
		if let Some(Ordering::Greater) = min.partial_cmp(val) {
			*val
		} else {
			min
		}
	})
}

fn spectrum_max_value(spectrum: &mut Spectrum) -> f64 {
	// f64 does not impl Ord because of NaN's and other weird edge cases.
	// We also depend here on the fact that spectrum values must be positive.
	spectrum.values().iter().fold(0.0, |max, val| {
		if let Some(Ordering::Less) = max.partial_cmp(val) {
			*val
		} else {
			max
		}
	})
}
