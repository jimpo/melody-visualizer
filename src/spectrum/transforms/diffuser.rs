use std::{any::Any, mem, sync::Arc};

use crate::spectrum::{LogHz, Spectrum, SpectrumBuffer, SpectrumTransform};
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
		println!("{} {} {}", max_log_freq, min_log_freq, params.samples());
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
		if !Arc::ptr_eq(self.buffer.params(), spectrum.params()) {
			self.buffer = SpectrumBuffer::new(spectrum.params().clone());
			self.regenerate_window();
		}
		let buffer = mem::take(&mut self.buffer);
		let new_spectrum = buffer.fill(|samples, _| {
			// Window is symmetric around origin and has odd size.
			let offset = -((self.window.len() / 2) as isize);
			convolve(samples, spectrum.values(), &self.window, offset);
		});
		self.buffer = spectrum.into_buffer();
		new_spectrum
	}

	fn upcast_any_ref(&self) -> &dyn Any {
		self
	}

	fn upcast_any_mut(&mut self) -> &mut dyn Any {
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
