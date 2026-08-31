pub mod decibel_converter;
pub mod diffuser;
pub mod volume_normalizer;

pub use decibel_converter::DecibelConverter;
pub use diffuser::Diffuser;
pub use volume_normalizer::VolumeNormalizer;

#[cfg(test)]
mod tests {
	use rand::{RngExt, SeedableRng, rngs::StdRng};

	use super::*;
	use crate::spectrum::{Spectrum, SpectrumTransform};
	use crate::test_support::{spectrum, spectrum_params};
	use crate::traits::Configurable;

	const BINS: usize = 64;

	/// One of every transform, at the extremes of its configured range as well
	/// as the value the app runs it at.
	fn every_transform() -> Vec<(&'static str, Box<dyn SpectrumTransform>)> {
		vec![
			(
				"diffuser (widest)",
				Box::new(Diffuser::new(diffuser::Config { width: 10.0 })),
			),
			(
				"diffuser (default)",
				Box::new(Diffuser::new(diffuser::Config { width: 1.0 / 24.0 })),
			),
			(
				"volume normalizer (slowest)",
				Box::new(VolumeNormalizer::new(volume_normalizer::Config {
					rate: 0.01,
				})),
			),
			(
				"volume normalizer (default)",
				Box::new(VolumeNormalizer::new(volume_normalizer::Config {
					rate: 0.1,
				})),
			),
			(
				"decibel converter",
				Box::new(DecibelConverter::new(decibel_converter::Config {
					min_level: 1.0e-6,
				})),
			),
		]
	}

	/// Spectra chosen to break a transform that divides, takes a logarithm, or
	/// accumulates: silence, the extremes of the range, denormals, and values
	/// spread over the whole exponent range.
	fn hostile_spectra() -> Vec<Vec<f64>> {
		let mut rng = StdRng::seed_from_u64(0);
		let mut spectra = vec![
			vec![0.0; BINS],
			vec![1.0e300; BINS],
			vec![f64::MIN_POSITIVE / 2.0; BINS],
			(0..BINS)
				.map(|index| if index % 2 == 0 { 1.0e300 } else { 1.0e-300 })
				.collect(),
		];
		spectra.extend((0..32).map(|_| {
			(0..BINS)
				.map(|_| 10.0f64.powf(rng.random_range(-300.0..300.0)))
				.collect()
		}));
		spectra
	}

	/// Every value in the spectrum is a real number.
	fn is_finite(spectrum: &Spectrum) -> bool {
		spectrum.values().iter().all(|value| value.is_finite())
	}

	#[test]
	fn no_transform_produces_a_nan_or_an_infinity() {
		let params = spectrum_params(BINS);
		let spectra = hostile_spectra();

		for (name, mut transform) in every_transform() {
			transform.set_params(&params);
			// One transform sees the whole sequence, so the state it carries
			// between spectra is exercised as well as each spectrum alone.
			for values in &spectra {
				let output = transform.transform(spectrum(values));

				assert!(
					is_finite(&output),
					"{name} produced {:?} from {:?}",
					output.values(),
					values,
				);
			}
		}
	}
}
