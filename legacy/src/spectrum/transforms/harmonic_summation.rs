use std::{any::Any, iter, mem, sync::Arc};

use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumParams, SpectrumTransform};
use crate::traits::Configurable;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Config {
	/// The weight of each harmonic relative to the one below it.
	pub decay: f64,
	/// How many harmonics a bin collects, the fundamental included.
	pub harmonics: usize,
}

/// Scores each bin by its own harmonic series: its value plus a decaying share
/// of the values at two, three, four… times its frequency.
///
/// The fundamental is the one frequency whose integer multiples all carry
/// energy, so it collects the whole series while a harmonic, whose own
/// multiples are sparse, collects little. This is the pitch salience function
/// of Salamon and Gómez (MELODIA).
///
/// The grid is log-spaced with a constant number of bins per octave, so
/// multiplying a frequency by `h` shifts the index by the constant
/// `round(bins_per_octave · log2(h))`. The rounding error is at most half a
/// bin. A harmonic above the top of the grid adds nothing.
#[derive(Debug)]
pub struct HarmonicSummation {
	config: Config,
	buffer: SpectrumBuffer,
	/// The index offset of each harmonic, the fundamental's 0 first.
	shifts: Vec<usize>,
}

impl Configurable for HarmonicSummation {
	type Config = Config;

	fn new(config: Config) -> Self {
		HarmonicSummation {
			config,
			buffer: SpectrumBuffer::default(),
			shifts: Vec::new(),
		}
	}

	fn set_config(&mut self, config: Config) {
		self.config = config;
		self.regenerate_shifts();
	}
}

impl HarmonicSummation {
	fn regenerate_shifts(&mut self) {
		self.shifts.clear();

		let params = self.buffer.params();
		if params.samples() < 2 {
			return;
		}

		let min_log_freq = params.min_log_freq().expect("params.samples() > 0");
		let max_log_freq = params.max_log_freq().expect("params.samples() > 0");
		let bins_per_octave = (params.samples() - 1) as f64 / (max_log_freq - min_log_freq);
		// A grid whose ends meet spans no octave, so a bin has no harmonics on
		// it but itself.
		if !bins_per_octave.is_finite() {
			self.shifts.push(0);
			return;
		}
		self.shifts.extend(
			(1..=self.config.harmonics)
				.map(|harmonic| (bins_per_octave * (harmonic as f64).log2()).round() as usize),
		);
	}
}

impl SpectrumTransform for HarmonicSummation {
	fn transform(&mut self, spectrum: Spectrum) -> Spectrum {
		let buffer = mem::take(&mut self.buffer);
		let new_spectrum = buffer.fill(|out, _| {
			let values = spectrum.values();
			let weights = iter::successors(Some(1.0), |weight| Some(weight * self.config.decay));
			for (index, out) in out.iter_mut().enumerate() {
				// The shifts ascend, so the first harmonic off the grid ends
				// the series.
				*out = iter::zip(&self.shifts, weights.clone())
					.map_while(|(shift, weight)| {
						values.get(index + shift).map(|value| weight * value)
					})
					.sum();
			}
		});
		self.buffer = spectrum.into_buffer();
		new_spectrum
	}

	fn set_params(&mut self, params: &Arc<SpectrumParams>) {
		self.buffer = SpectrumBuffer::new(params.clone());
		self.regenerate_shifts();
	}

	fn as_any_mut(&mut self) -> &mut dyn Any {
		self
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use crate::app::Config as AppConfig;
	use crate::spectrum::{TransformChain, TransformId};
	use crate::test_support::{spectrum, spectrum_params};

	const DEFAULT: Config = Config {
		decay: 0.8,
		harmonics: 8,
	};

	fn harmonic_summation(params: &Arc<SpectrumParams>) -> HarmonicSummation {
		let mut transform = HarmonicSummation::new(DEFAULT);
		transform.set_params(params);
		transform
	}

	#[test]
	fn a_harmonic_series_scores_highest_at_its_fundamental() {
		let params = spectrum_params(1196);
		let mut transform = harmonic_summation(&params);
		let fundamental = 100;
		let mut values = vec![0.0; params.samples()];
		for shift in &transform.shifts[..4] {
			values[fundamental + shift] = 1.0;
		}

		let output = transform.transform(spectrum(&values));

		let loudest = (0..output.values().len())
			.max_by(|&a, &b| output.values()[a].total_cmp(&output.values()[b]))
			.unwrap();
		assert_eq!(
			loudest,
			fundamental,
			"the series scores {:?}",
			output.values()
		);
		assert!(output.values()[fundamental] > output.values()[fundamental + transform.shifts[1]]);
	}

	#[test]
	fn a_bin_whose_harmonics_fall_off_the_grid_keeps_only_its_own_weight() {
		let params = spectrum_params(1196);
		let mut transform = harmonic_summation(&params);
		let mut values = vec![1.0; params.samples()];
		values[1195] = 0.5;

		let output = transform.transform(spectrum(&values));

		assert_eq!(output.values()[1195], 0.5);
	}

	#[test]
	fn the_second_harmonic_is_one_octave_of_bins_up() {
		let config = AppConfig::default();
		let transform = harmonic_summation(&Arc::new(config.spectrum_params()));

		assert_eq!(transform.shifts[0], 0);
		assert_eq!(transform.shifts[1], config.samples_per_octave);
	}

	#[test]
	fn a_grid_spanning_no_octave_passes_the_spectrum_through() {
		let params = Arc::new(SpectrumParams::exp_spaced(4, 440.0, 440.0));
		let mut transform = harmonic_summation(&params);
		let values = [1.0, 2.0, 3.0, 4.0];

		let output = transform
			.transform(SpectrumBuffer::new(params).fill(|data, _| data.copy_from_slice(&values)));

		assert_eq!(output.values(), values);
	}

	#[test]
	fn bypassed_it_is_the_identity() {
		let params = spectrum_params(1196);
		let mut chain = TransformChain::default();
		chain.insert(0, TransformId(0), Box::new(HarmonicSummation::new(DEFAULT)));
		chain.set_params(&params);
		chain.set_enabled(TransformId(0), false);
		let values = (0..params.samples())
			.map(|index| (index % 7) as f64)
			.collect::<Vec<_>>();

		assert_eq!(chain.apply(spectrum(&values)).values(), values);
	}
}
