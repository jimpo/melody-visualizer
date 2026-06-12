pub mod generators;
pub mod renderer;
pub mod transforms;

use std::{
	any::Any,
	fmt::{self, Debug},
	sync::Arc,
	time::Duration,
};

pub type Hz = f64;
pub type LogHz = f64;

#[derive(Debug, Clone)]
pub struct SpectrumBuffer {
	params: Arc<SpectrumParams>,
	data: Vec<f64>,
}

impl SpectrumBuffer {
	pub fn new(params: Arc<SpectrumParams>) -> Self {
		let data = vec![0.0; params.samples()];
		SpectrumBuffer { params, data }
	}

	pub fn fill(mut self, f: impl Fn(&mut [f64], &SpectrumParams)) -> Spectrum {
		f(&mut self.data, &*self.params);
		Spectrum { buffer: self }
	}

	pub fn params(&self) -> &Arc<SpectrumParams> {
		&self.params
	}
}

impl Default for SpectrumBuffer {
	fn default() -> Self {
		Self::new(Arc::new(SpectrumParams::default()))
	}
}

#[derive(Clone, Default)]
pub struct Spectrum {
	buffer: SpectrumBuffer,
}

impl Spectrum {
	pub fn into_buffer(self) -> SpectrumBuffer {
		self.buffer
	}

	pub fn values(&self) -> &[f64] {
		&self.buffer.data
	}
	pub fn values_mut(&mut self) -> &mut [f64] {
		&mut self.buffer.data
	}
	pub fn params(&self) -> &Arc<SpectrumParams> {
		self.buffer.params()
	}
}

#[derive(Clone, Default, PartialEq)]
pub struct SpectrumParams {
	frequencies: Vec<f64>,
	log_frequencies: Vec<f64>,
}

impl SpectrumParams {
	pub fn exp_spaced(samples: usize, min_freq: f64, max_freq: f64) -> Self {
		// there must be at least two samples, one at min_freq and one at max_freq
		assert!(samples >= 2);

		let mut frequencies = Vec::with_capacity(samples);
		let mut log_frequencies = Vec::with_capacity(samples);

		let min_log_freq = min_freq.log2();
		let max_log_freq = max_freq.log2();
		for i in 0..samples {
			let interp_ratio = (i as f64) / ((samples - 1) as f64);
			let log_freq = min_log_freq * (1.0 - interp_ratio) + max_log_freq * interp_ratio;
			let freq = log_freq.exp2();

			frequencies.push(freq);
			log_frequencies.push(log_freq);
		}
		SpectrumParams {
			frequencies,
			log_frequencies,
		}
	}

	pub fn samples(&self) -> usize {
		self.frequencies.len()
	}

	pub fn frequencies(&self) -> &[f64] {
		&self.frequencies
	}

	pub fn log_frequencies(&self) -> &[f64] {
		&self.log_frequencies
	}

	pub fn min_freq(&self) -> Option<f64> {
		self.frequencies.first().cloned()
	}

	pub fn max_freq(&self) -> Option<f64> {
		self.frequencies.last().cloned()
	}

	pub fn min_log_freq(&self) -> Option<f64> {
		self.log_frequencies.first().cloned()
	}

	pub fn max_log_freq(&self) -> Option<f64> {
		self.log_frequencies.last().cloned()
	}
}

impl fmt::Debug for SpectrumParams {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("SpectrumParams")
			.field("min_freq", &self.min_freq())
			.field("max_freq", &self.max_freq())
			.field("samples", &self.samples())
			.finish()
	}
}

pub trait SpectrumGenerator: Debug {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum;
	fn interval(&self) -> Duration;
}

pub trait SpectrumTransform: Debug {
	fn transform(&mut self, spectrum: Spectrum) -> Spectrum;

	fn upcast_any_ref(&self) -> &dyn Any;
	fn upcast_any_mut(&mut self) -> &mut dyn Any;
}
