#[derive(Clone, Default)]
pub struct SpectrumBuffer {
	params: SpectrumParams,
}

impl SpectrumBuffer {
}

#[derive(Clone, Default)]
pub struct Spectrum {
	buffer: SpectrumBuffer,
}

impl Spectrum {
	pub fn into_buffer(self) -> SpectrumBuffer {
		self.buffer
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
			log_frequencies
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
}
