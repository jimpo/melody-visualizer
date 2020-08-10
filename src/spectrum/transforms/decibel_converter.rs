use std::any::Any;

use crate::spectrum::{Spectrum, SpectrumTransform};
use crate::traits::Configurable;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
	pub min_level: f64,
}

// This basically is a convolution of a small triangular window with the spectrum.
#[derive(Debug)]
pub struct DecibelConverter {
	config: Config,
	log_min_level: f64,
}

impl Configurable for DecibelConverter {
	type Config = Config;

	fn new(config: Config) -> Self {
		let log_min_level = config.min_level.log10();
		DecibelConverter {
			config,
			log_min_level,
		}
	}

	fn set_config(&mut self, config: Config) {
		self.config = config;
		self.log_min_level = self.config.min_level.log10();
	}
}

impl SpectrumTransform for DecibelConverter {
	fn transform(&mut self, mut spectrum: Spectrum) -> Spectrum {
		for val in spectrum.values_mut().iter_mut() {
			let new_val = if *val > self.config.min_level {
				(*val).log10() - self.log_min_level
			} else {
				0.0
			};
			*val = new_val;
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
