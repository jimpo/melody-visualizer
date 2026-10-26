use std::any::Any;

use crate::spectrum::{Spectrum, SpectrumTransform};
use crate::traits::Configurable;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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

	fn as_any_mut(&mut self) -> &mut dyn Any {
		self
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use crate::test_support::spectrum;

	const FLOOR: f64 = 1.0e-6;

	fn converter() -> DecibelConverter {
		DecibelConverter::new(Config { min_level: FLOOR })
	}

	#[test]
	fn values_at_or_below_the_floor_map_to_zero() {
		let output = converter().transform(spectrum(&[0.0, 1.0e-9, FLOOR, 1.0e-3]));

		assert_eq!(&output.values()[..3], [0.0, 0.0, 0.0]);
		assert!(output.values()[3] > 0.0);
	}

	#[test]
	fn each_decade_above_the_floor_counts_one() {
		let output = converter().transform(spectrum(&[1.0e-5, 1.0e-4, 1.0e-2]));

		for (value, decades) in std::iter::zip(output.values(), [1.0, 2.0, 4.0]) {
			assert!(
				(value - decades).abs() < 1e-12,
				"{value} decades above the floor"
			);
		}
	}

	#[test]
	fn the_conversion_is_monotonic() {
		let input = [0.0, FLOOR, 1.0e-5, 1.0e-4, 1.0e-2, 1.0, 1.0e3];

		let output = converter().transform(spectrum(&input));

		assert!(
			output.values().windows(2).all(|pair| pair[0] <= pair[1]),
			"a louder bin never converts to a quieter one: {:?}",
			output.values(),
		);
	}
}
