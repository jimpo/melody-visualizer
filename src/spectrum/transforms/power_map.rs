use std::any::Any;

use crate::spectrum::{Spectrum, SpectrumTransform};
use crate::traits::Configurable;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Config {
	/// The power every bin is raised to. Above 1 dims the bins below the peak,
	/// 1 is the identity, and below 1 lifts them.
	pub exponent: f64,
}

/// Raises every bin to a configured exponent, which sets the display's contrast.
///
/// A bin at a ratio `r` of the loudest comes out at `r^exponent` of it, so after
/// the volume normalizer the peak stays at full brightness while quieter bins
/// dim or lift.
#[derive(Debug)]
pub struct PowerMap {
	config: Config,
}

impl Configurable for PowerMap {
	type Config = Config;

	fn new(config: Config) -> Self {
		PowerMap { config }
	}

	fn set_config(&mut self, config: Config) {
		self.config = config;
	}
}

impl SpectrumTransform for PowerMap {
	fn transform(&mut self, mut spectrum: Spectrum) -> Spectrum {
		let exponent = self.config.exponent;
		for value in spectrum.values_mut().iter_mut() {
			// Saturates rather than overflowing to infinity on a huge input.
			*value = value.powf(exponent).min(f64::MAX);
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

	use crate::spectrum::{TransformChain, TransformId};
	use crate::test_support::spectrum;

	fn power_map(exponent: f64) -> PowerMap {
		PowerMap::new(Config { exponent })
	}

	#[test]
	fn exponent_one_is_the_identity() {
		let values = [0.0, 0.25, 0.5, 1.0, 3.0];

		let output = power_map(1.0).transform(spectrum(&values));

		assert_eq!(output.values(), values);
	}

	#[test]
	fn exponent_two_squares_every_bin() {
		let output = power_map(2.0).transform(spectrum(&[0.0, 0.5, 1.0, 3.0]));

		assert_eq!(
			output.values(),
			[0.0, 0.25, 1.0, 9.0],
			"a bin at half the peak lands at a quarter",
		);
	}

	#[test]
	fn bypassed_it_is_the_identity() {
		let id = TransformId(0);
		let mut chain = TransformChain::default();
		chain.insert(0, id, Box::new(power_map(2.0)));
		chain.set_enabled(id, false);

		let output = chain.apply(spectrum(&[0.0, 0.5, 1.0, 3.0]));

		assert_eq!(output.values(), [0.0, 0.5, 1.0, 3.0]);
	}
}
