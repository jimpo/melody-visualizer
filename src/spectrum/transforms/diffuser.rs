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

	use crate::test_support::{spectrum, spectrum_params};

	#[test]
	fn an_impulse_comes_out_as_the_window() {
		let mut diffuser = Diffuser::new(Config { width: 1.0 });
		diffuser.set_params(&spectrum_params(129));
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
}
