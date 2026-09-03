use cairo::{
	Mesh,
	MeshCorner::{MeshCorner0, MeshCorner1, MeshCorner2, MeshCorner3},
};
use palette::{Hsv, IntoColor, RgbHue, encoding::Srgb, rgb::Rgb};
use std::any::Any;
use std::cmp;
use std::collections::VecDeque;
use std::f64::consts::PI;
use std::sync::Arc;

use crate::error::Error;
use crate::graphic::{Graphic, GraphicBuffer, GraphicGenerator};
use crate::spectrum::{LogHz, Spectrum, SpectrumParams};
use crate::traits::Configurable;

#[derive(Debug)]
pub struct SpiralGenerator {
	x_max: i32,
	y_max: i32,
	config: Config,
	params: Arc<SpectrumParams>,
	edges: Vec<SegmentEdge>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
	/// Pixels between the outermost ring and the nearer edge of the surface.
	pub outer_pad: f64,
	/// Radius in pixels of the empty disc at the centre.
	pub center_pad: f64,
	/// The pitch the colour wheel and the angle origin are aligned to, as
	/// **log₂(Hz)** — the unit [`Note::log_frequency`](crate::note::Note::log_frequency)
	/// and [`SpectrumParams::log_frequencies`] are both in, not Hz.
	///
	/// Every pitch class an octave apart from it sits on the same spoke, so only
	/// the fractional part of the difference matters; a value in the wrong unit
	/// still draws a spiral, just one keyed to an arbitrary pitch.
	pub key_log_freq: LogHz,
}

#[derive(Debug)]
struct SegmentEdge {
	hue: RgbHue<f64>,
	saturation: f64,
	x_inner: f64,
	y_inner: f64,
	x_center: f64,
	y_center: f64,
	x_outer: f64,
	y_outer: f64,
}

impl Configurable for SpiralGenerator {
	type Config = Config;

	fn new(config: Config) -> Self {
		SpiralGenerator {
			x_max: 0,
			y_max: 0,
			config,
			params: Arc::new(SpectrumParams::default()),
			edges: Vec::new(),
		}
	}

	fn set_config(&mut self, config: Config) {
		self.config = config;
		self.regenerate();
	}
}

impl SpiralGenerator {
	fn regenerate(&mut self) {
		if self.params.log_frequencies().is_empty() {
			self.edges.clear();
			return;
		}

		let min_log_freq = self
			.params
			.min_log_freq()
			.expect("params.log_frequencies() is not empty");
		let max_log_freq = self
			.params
			.max_log_freq()
			.expect("params.log_frequencies() is not empty");

		self.edges.clear();
		self.edges.reserve(self.params.log_frequencies().len());

		let r_min = self.config.center_pad;
		let r_max =
			(cmp::min(self.x_max, self.y_max) as f64 / 2.0 - self.config.outer_pad).max(r_min);
		let r_scale = (r_max - r_min) / (max_log_freq - min_log_freq);
		let x_origin = self.x_max as f64 / 2.0;
		let y_origin = self.y_max as f64 / 2.0;

		for log_freq in self.params.log_frequencies().iter().cloned() {
			// Compute the fraction of an octave away from the key frequency on a log scale.
			let log_freq_delta = log_freq - self.config.key_log_freq;
			let log_freq_delta_norm = log_freq_delta - log_freq_delta.floor();

			let r = r_min + r_scale * (log_freq - min_log_freq);
			let thickness = r_scale * 0.1; // 10% of the distance between octaves.
			let theta = log_freq_delta_norm * 2.0 * PI;
			let sin_theta = theta.sin();
			let cos_theta = theta.cos();

			self.edges.push(SegmentEdge {
				hue: RgbHue::from_radians(theta),
				saturation: 0.80,
				x_inner: x_origin + sin_theta * (r - thickness),
				y_inner: y_origin - cos_theta * (r - thickness),
				x_center: x_origin + sin_theta * r,
				y_center: y_origin - cos_theta * r,
				x_outer: x_origin + sin_theta * (r + thickness),
				y_outer: y_origin - cos_theta * (r + thickness),
			});
		}
	}
}

impl GraphicGenerator for SpiralGenerator {
	fn generate(
		&mut self,
		buffer: GraphicBuffer,
		spectrum_history: &VecDeque<Spectrum>,
	) -> Result<Graphic, Error> {
		let spectrum = spectrum_history.front().map(|spectrum| spectrum.values());

		buffer.draw(|ctx| {
			ctx.set_source_rgb(0.0, 0.0, 0.0);
			ctx.rectangle(0.0, 0.0, self.x_max as f64, self.y_max as f64);
			ctx.fill()?;

			if self.edges.is_empty() {
				return Ok(());
			}

			let mesh = Mesh::new();

			for i in 1..self.edges.len() {
				let edge1 = &self.edges[i - 1];
				let edge2 = &self.edges[i];

				let value1 = 0.2 + 0.8 * spectrum.map_or(0.0, |spectrum| spectrum[i - 1].min(1.0));
				let value2 = 0.2 + 0.8 * spectrum.map_or(0.0, |spectrum| spectrum[i].min(1.0));

				let color1: Rgb<Srgb, f64> =
					<Hsv<Srgb, f64>>::new(edge1.hue, edge1.saturation, value1).into_color();
				let color2: Rgb<Srgb, f64> =
					<Hsv<Srgb, f64>>::new(edge2.hue, edge2.saturation, value2).into_color();

				mesh.begin_patch();
				mesh.line_to(edge1.x_center, edge1.y_center);
				mesh.line_to(edge2.x_center, edge2.y_center);
				mesh.line_to(edge2.x_outer, edge2.y_outer);
				mesh.line_to(edge1.x_outer, edge1.y_outer);
				mesh.set_corner_color_rgb(MeshCorner0, color1.red, color1.green, color1.blue);
				mesh.set_corner_color_rgb(MeshCorner1, color2.red, color2.green, color2.blue);
				mesh.set_corner_color_rgb(MeshCorner2, 0.0, 0.0, 0.0);
				mesh.set_corner_color_rgb(MeshCorner3, 0.0, 0.0, 0.0);
				mesh.end_patch();

				mesh.begin_patch();
				mesh.line_to(edge2.x_center, edge2.y_center);
				mesh.line_to(edge1.x_center, edge1.y_center);
				mesh.line_to(edge1.x_inner, edge1.y_inner);
				mesh.line_to(edge2.x_inner, edge2.y_inner);
				mesh.set_corner_color_rgb(MeshCorner0, color1.red, color1.green, color1.blue);
				mesh.set_corner_color_rgb(MeshCorner1, color2.red, color2.green, color2.blue);
				mesh.set_corner_color_rgb(MeshCorner2, 0.0, 0.0, 0.0);
				mesh.set_corner_color_rgb(MeshCorner3, 0.0, 0.0, 0.0);
				mesh.end_patch();
			}

			ctx.set_source(&*mesh)?;
			ctx.paint()?;

			Ok(())
		})
	}

	fn set_params(&mut self, params: &Arc<SpectrumParams>) {
		self.params = params.clone();
		self.regenerate();
	}

	fn set_size(&mut self, width: i32, height: i32) {
		self.x_max = width;
		self.y_max = height;
		self.regenerate();
	}

	fn history_len(&self) -> usize {
		1
	}

	fn as_any_mut(&mut self) -> &mut dyn Any {
		self
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use crate::note;

	const SIZE: i32 = 400;

	/// A generator on a semitone grid from C3 to C6, keyed to C.
	///
	/// Three octaves at twelve bins each puts a bin on every semitone, so bins
	/// 0, 12, 24 and 36 are the pitch class the key names.
	fn keyed_to_c() -> SpiralGenerator {
		let params = Arc::new(SpectrumParams::exp_spaced(
			37,
			note!(C, 3).frequency(),
			note!(C, 6).frequency(),
		));
		let mut generator = SpiralGenerator::new(Config {
			outer_pad: 20.0,
			center_pad: 50.0,
			key_log_freq: note!(C, 4).log_frequency(),
		});
		generator.set_size(SIZE, SIZE);
		generator.set_params(&params);
		generator
	}

	/// The angle of `edge` about the surface centre, clockwise from straight up.
	///
	/// In `(-π, π]`, so a spoke a hair either side of the origin reads as a small
	/// angle rather than one close to a full turn.
	fn angle(edge: &SegmentEdge) -> f64 {
		let origin = SIZE as f64 / 2.0;
		(edge.x_center - origin).atan2(origin - edge.y_center)
	}

	#[test]
	fn pitch_class_c_sits_at_the_angle_origin_in_every_octave() {
		let generator = keyed_to_c();
		for index in [0, 12, 24, 36] {
			let angle = angle(&generator.edges[index]);
			assert!(
				angle.abs() < 1e-6,
				"bin {} is a C, so it belongs at angle 0, not {}",
				index,
				angle,
			);
		}
	}

	#[test]
	fn frequencies_an_octave_apart_share_an_angle_and_a_hue() {
		let generator = keyed_to_c();
		// A tritone from the key, so neither the angle nor the hue sits on the
		// wrap-around where two representations of the same direction differ.
		let [first, second, third] = [6, 18, 30].map(|index: usize| &generator.edges[index]);
		for other in [second, third] {
			assert!((angle(first) - angle(other)).abs() < 1e-6);
			assert!(
				(first.hue.into_positive_degrees() - other.hue.into_positive_degrees()).abs()
					< 1e-6,
			);
		}
	}
}
