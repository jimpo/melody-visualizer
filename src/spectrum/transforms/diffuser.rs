use std::{any::Any, mem, sync::Arc};

use crate::spectrum::{LogHz, Spectrum, SpectrumBuffer, SpectrumParams, SpectrumTransform};
use crate::traits::Configurable;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
	pub width: LogHz,
}

// This basically is a convolution of a small triangular window with the spectrum.
#[derive(Debug)]
pub struct Diffuser {
	config: Config,
	buffer: SpectrumBuffer,
	window: Vec<f64>,
}

impl Configurable for Diffuser {
	type Config = Config;

	fn new(config: Config) -> Self {
		Diffuser {
			config,
			buffer: SpectrumBuffer::default(),
			window: Vec::new(),
		}
	}

	fn set_config(&mut self, config: Config) {
		self.config = config;
		self.regenerate_window();
	}
}

impl Diffuser {
	fn regenerate_window(&mut self) {
		self.window.clear();

		let params = self.buffer.params();
		if params.samples() == 0 {
			return;
		}

		let min_log_freq = params.min_log_freq().expect("params.samples() > 0");
		let max_log_freq = params.max_log_freq().expect("params.samples() > 0");
		let dist_between_samples = (max_log_freq - min_log_freq) / (params.samples() - 1) as f64;
		assert!(dist_between_samples.is_finite() && dist_between_samples.is_sign_positive());

		let half_width = self.config.width / 2.0;
		let samples_per_side = (half_width / dist_between_samples) as usize;
		self.window.resize(samples_per_side * 2 + 1, 0.0);

		self.window[samples_per_side] = 1.0;
		for i in 0..samples_per_side {
			let val = 1.0 - (i + 1) as f64 * dist_between_samples / half_width;
			self.window[samples_per_side - (i + 1)] = val;
			self.window[samples_per_side + (i + 1)] = val;
		}

		normalize(&mut self.window);
	}
}

impl SpectrumTransform for Diffuser {
	fn transform(&mut self, spectrum: Spectrum) -> Spectrum {
		let buffer = mem::take(&mut self.buffer);
		let new_spectrum = buffer.fill(|samples, _| {
			// The window has odd size and is symmetric about its centre, so
			// indexing it from its centre spans [-half, half] and leaves the
			// weighted average sitting on the bin it was taken around.
			let half_width = (self.window.len() / 2) as isize;
			convolve(samples, spectrum.values(), &self.window, half_width);
		});
		self.buffer = spectrum.into_buffer();
		new_spectrum
	}

	fn set_params(&mut self, params: &Arc<SpectrumParams>) {
		self.buffer = SpectrumBuffer::new(params.clone());
		self.regenerate_window();
	}

	fn as_any_mut(&mut self) -> &mut dyn Any {
		self
	}
}

// g takes values in [-g_offset, -g_offset + g.len())
fn convolve(out: &mut [f64], f: &[f64], g: &[f64], g_offset: isize) {
	assert_eq!(out.len(), f.len());
	for (n, out_n) in out.iter_mut().enumerate() {
		*out_n = 0.0;
		for (i, g_i) in g.iter().enumerate() {
			let m = i as isize - g_offset;
			let j = n as isize - m;
			if j >= 0 && (j as usize) < f.len() {
				*out_n += f[j as usize] * g_i;
			}
		}
	}
}

fn normalize(xs: &mut [f64]) {
	let sum: f64 = xs.iter().sum();
	xs.iter_mut().for_each(|x| *x /= sum);
}

#[cfg(test)]
mod tests {
	use super::*;

	use std::iter;

	use crate::test_support::{spectrum, spectrum_params};

	/// A diffuser of the given width, on a grid of `samples` bins.
	fn diffuser(width: LogHz, samples: usize) -> Diffuser {
		let mut diffuser = Diffuser::new(Config { width });
		diffuser.set_params(&spectrum_params(samples));
		diffuser
	}

	#[test]
	fn an_impulse_comes_out_as_the_window() {
		let mut diffuser = diffuser(1.0, 129);
		let window = diffuser.window.clone();

		// A single loud bin, far enough from either end for the window to fit
		// beside it.
		let mut values = vec![0.0; 129];
		values[64] = 1.0;
		let output = diffuser.transform(spectrum(&values));

		let half_width = window.len() / 2;
		assert_eq!(
			&output.values()[64 - half_width..=64 + half_width],
			window,
			"the smoothed bin stays where it was, with the window around it",
		);
	}

	#[test]
	fn a_zero_width_diffuser_is_the_identity() {
		let mut diffuser = diffuser(0.0, 5);

		let output = diffuser.transform(spectrum(&[0.0, 1.0, 0.0, 2.0, 0.0]));

		assert_eq!(
			output.values(),
			[0.0, 1.0, 0.0, 2.0, 0.0],
			"a window narrower than a bin collapses to [1.0]",
		);
	}

	#[test]
	fn the_window_is_symmetric_and_sums_to_one() {
		let diffuser = diffuser(1.0, 129);

		let window = &diffuser.window;
		assert!(window.len() > 1, "an octave spans more than one bin");
		assert_eq!(window.len() % 2, 1, "the window is centred on a bin");
		for (left, right) in iter::zip(window, window.iter().rev()) {
			assert_eq!(left, right, "the window is symmetric about its centre");
		}
		assert!(
			(window.iter().sum::<f64>() - 1.0).abs() < 1e-12,
			"a window summing to 1 makes the transform a weighted average",
		);
	}

	#[test]
	fn total_power_is_preserved() {
		let mut diffuser = diffuser(1.0, 129);
		// Away from the ends, where the window would hang off the edge and the
		// power under it would be dropped.
		let mut values = vec![0.0; 129];
		for (offset, value) in values[40..90].iter_mut().enumerate() {
			*value = (offset % 7) as f64;
		}
		let power = values.iter().sum::<f64>();

		let output = diffuser.transform(spectrum(&values));

		assert!((output.values().iter().sum::<f64>() - power).abs() < 1e-12);
	}
}
