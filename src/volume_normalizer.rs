use std::cmp::Ordering;

use crate::spectrum_renderer::SpectrumTransform;
use crate::spectrum::Spectrum;

#[derive(Debug)]
pub struct VolumeNormalizer {
	rate: f64,
	max_value: f64,
}

impl VolumeNormalizer {
	pub fn new(rate: f64) -> Self {
		VolumeNormalizer {
			rate,
			max_value: 0.0,
		}
	}
}

impl SpectrumTransform for VolumeNormalizer {
	fn transform(&mut self, mut spectrum: Spectrum) -> Spectrum {
		if spectrum.params().samples() == 0 {
			return spectrum;
		}

		let spectrum_max = spectrum_max_value(&mut spectrum);
		self.max_value = (1.0 - self.rate) * self.max_value + self.rate * spectrum_max;

		if spectrum_max != 0.0 {
			for value in spectrum.values_mut().iter_mut() {
				*value /= self.max_value;
			}
		}
		spectrum
	}
}

fn spectrum_max_value(spectrum: &mut Spectrum) -> f64 {
	// f64 does not impl Ord because of NaN's and other weird edge cases.
	// We also depend here on the fact that spectrum values must be positive.
	spectrum
		.values()
		.iter()
		.fold(0.0, |max, val| {
			if let Some(Ordering::Less) = max.partial_cmp(val) {
				*val
			} else {
				max
			}
		})
}