use std::{any::Any, cmp::Ordering};

use crate::spectrum::{Spectrum, SpectrumTransform};
use crate::traits::Configurable;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
	/// How fast the running peak follows the spectrum, per frame, in `(0, 1]`.
	///
	/// A rate of 1 tracks the peak exactly; smaller values smooth it over
	/// roughly `1 / rate` frames.
	pub rate: f64,
}

/// Scales a spectrum by a peak that follows the loudest bin over time, so that
/// quiet passages fill the same range as loud ones.
///
/// The output is a fraction of that running peak: nominally `[0, ~1]`, and above
/// 1 for a transient louder than the peak has caught up with. See the value-range
/// contract in ARCHITECTURE.md.
#[derive(Debug)]
pub struct VolumeNormalizer {
	config: Config,
	/// The running peak, or 0 until a spectrum with a peak seeds it.
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
		let spectrum_max = spectrum_max_value(&spectrum);
		let rate = self.config.rate;
		self.max_value = if self.max_value > 0.0 {
			(1.0 - rate) * self.max_value + rate * spectrum_max
		} else {
			// Seed the peak from the first spectrum that has one. Blending
			// against 0 instead would scale that spectrum by `1 / rate`, and at
			// rate 0 would leave the peak at 0 forever.
			spectrum_max
		};

		// A peak of 0 means every value is 0: there is nothing to scale, and
		// dividing would produce NaN.
		if self.max_value > 0.0 {
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

fn spectrum_max_value(spectrum: &Spectrum) -> f64 {
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

#[cfg(test)]
mod tests {
	use super::*;

	use crate::spectrum::Spectrum;
	use crate::test_support::spectrum;

	fn normalizer(rate: f64) -> VolumeNormalizer {
		VolumeNormalizer::new(Config { rate })
	}

	fn is_finite(spectrum: &Spectrum) -> bool {
		spectrum.values().iter().all(|value| value.is_finite())
	}

	#[test]
	fn the_first_frame_is_not_amplified() {
		let mut normalizer = normalizer(0.1);

		let output = normalizer.transform(spectrum(&[1.0, 2.0, 4.0, 2.0]));

		assert_eq!(
			output.values(),
			[0.25, 0.5, 1.0, 0.5],
			"the first spectrum seeds the peak, so it comes out scaled to 1.0",
		);
	}

	#[test]
	fn rate_zero_produces_no_infinities() {
		let mut normalizer = normalizer(0.0);

		// The peak is frozen at whatever seeded it, and never divides by zero.
		let first = normalizer.transform(spectrum(&[1.0, 2.0, 4.0, 2.0]));
		let second = normalizer.transform(spectrum(&[8.0, 8.0, 8.0, 8.0]));

		assert!(is_finite(&first) && is_finite(&second));
		assert_eq!(first.values(), [0.25, 0.5, 1.0, 0.5]);
		assert_eq!(second.values(), [2.0, 2.0, 2.0, 2.0]);
	}

	#[test]
	fn silence_produces_no_nans() {
		let mut normalizer = normalizer(0.1);

		let output = normalizer.transform(spectrum(&[0.0; 4]));

		assert_eq!(output.values(), [0.0; 4]);
		assert!(is_finite(&normalizer.transform(Spectrum::default())));
	}

	#[test]
	fn the_peak_converges_under_constant_input() {
		let rate = 0.1;
		let mut normalizer = normalizer(rate);

		normalizer.transform(spectrum(&[1.0, 2.0]));
		assert_eq!(
			normalizer.max_value, 2.0,
			"the first spectrum seeds the peak"
		);

		// The gap between the running peak and the input peak decays by
		// `1 - rate` per frame, so 1/rate frames leave about `1/e` of it.
		let frames = (1.0 / rate).round() as usize;
		for _ in 0..frames {
			normalizer.transform(spectrum(&[0.5, 1.0]));
		}
		let remaining_gap = normalizer.max_value - 1.0;
		assert!(
			(remaining_gap - std::f64::consts::E.recip()).abs() < 0.05,
			"after 1/rate frames about 1/e of the gap is left, got {remaining_gap}",
		);

		let output = (0..10 * frames)
			.map(|_| normalizer.transform(spectrum(&[0.5, 1.0])))
			.last()
			.expect("rate is at most 1, so there is at least one frame");
		assert!(
			(output.values()[1] - 1.0).abs() < 1e-3,
			"the peak settles on the input peak, putting its loudest bin at 1.0",
		);
	}
}
