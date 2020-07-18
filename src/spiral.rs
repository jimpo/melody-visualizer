use cairo::{Mesh, MeshCorner::{MeshCorner0, MeshCorner1, MeshCorner2, MeshCorner3}};
use palette::{encoding::Srgb, IntoColor, Hsv, RgbHue};
use std::cmp;
use std::collections::VecDeque;
use std::f64::consts::PI;
use std::sync::Arc;

use crate::graphic_renderer::GraphicGenerator;
use crate::graphic::{GraphicBuffer, Graphic};
use crate::note::{Note, PitchClass};
use crate::error::Error;
use crate::spectrum::{Spectrum, SpectrumParams};

#[derive(Debug)]
pub struct SpiralGenerator {
	x_max: i32,
	y_max: i32,
	outer_pad: f64,
	center_pad: f64,
	key: PitchClass,
	params: Arc<SpectrumParams>,
	edges: Vec<SegmentEdge>,
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

impl SpiralGenerator {
	pub fn new(outer_pad: f64, center_pad: f64, key: PitchClass) -> Self {
		SpiralGenerator {
			x_max: 0,
			y_max: 0,
			outer_pad,
			center_pad,
			key,
			params: Arc::new(SpectrumParams::default()),
			edges: Vec::new(),
		}
	}

	fn regenerate(&mut self, params: &Arc<SpectrumParams>, x_max: i32, y_max: i32) {
		self.x_max = x_max;
		self.y_max = y_max;
		self.params = params.clone();

		if params.log_frequencies().is_empty() {
			self.edges.clear();
			return;
		}

		let min_log_freq = self.params.min_log_freq()
			.expect("params.log_frequencies() is not empty");
		let max_log_freq = self.params.max_log_freq()
			.expect("params.log_frequencies() is not empty");
		let key_log_freq = Note { octave: 0, pitch_class: self.key }.log_frequency();

		self.edges.clear();
		self.edges.reserve(params.log_frequencies().len());

		let r_min = self.center_pad;
		let r_max = (cmp::min(self.x_max, self.y_max) as f64 / 2.0 - self.outer_pad).max(r_min);
		let r_scale = (r_max - r_min) / (max_log_freq - min_log_freq);
		let x_origin = self.x_max as f64 / 2.0;
		let y_origin = self.y_max as f64 / 2.0;

		for log_freq in params.log_frequencies().iter().cloned() {
			// Compute the fraction of an octave away from the key frequency on a log scale.
			let log_freq_delta = log_freq - key_log_freq;
			let log_freq_delta_norm = log_freq_delta - log_freq_delta.floor();

			let r = r_min + r_scale * (log_freq - min_log_freq);
			let thickness = r_scale * 0.1; // 10% of the distance between octaves.
			let theta = log_freq_delta_norm * 2.0 * PI;
			let sin_theta = theta.sin();
			let cos_theta = theta.cos();

			self.edges.push(SegmentEdge {
				hue: RgbHue::from_radians(theta),
				saturation: 0.80,
				x_inner:  x_origin + sin_theta * (r - thickness),
				y_inner:  y_origin - cos_theta * (r - thickness),
				x_center: x_origin + sin_theta * r,
				y_center: y_origin - cos_theta * r,
				x_outer:  x_origin + sin_theta * (r + thickness),
				y_outer:  y_origin - cos_theta * (r + thickness),
			});
		}
	}
}

impl GraphicGenerator for SpiralGenerator {
	fn generate(
		&mut self,
		buffer: GraphicBuffer,
		params: &Arc<SpectrumParams>,
		spectrum_history: &VecDeque<Spectrum>,
	) -> Result<Graphic, Error>
	{
		let x_max = buffer.width();
		let y_max = buffer.height();

		if !(Arc::ptr_eq(&self.params, params) && x_max == self.x_max && y_max == self.y_max) {
			self.regenerate(params, x_max, y_max);
		}

		buffer.draw(|ctx| {
			ctx.set_source_rgb(0.0, 0.0, 0.0);
			ctx.rectangle(0.0, 0.0, self.x_max as f64, self.y_max as f64);
			ctx.fill();

			if self.edges.is_empty() {
				return Ok(());
			}

			let mesh = Mesh::new();

			for i in 1..self.edges.len() {
				let edge1 = &self.edges[i - 1];
				let edge2 = &self.edges[i];

				let value = 0.9;
				let color1 = <Hsv<Srgb, f64>>::new(edge1.hue, edge1.saturation, value)
					.into_rgb::<Srgb>();
				let color2 = <Hsv<Srgb, f64>>::new(edge2.hue, edge2.saturation, value)
					.into_rgb::<Srgb>();

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

			ctx.set_source(&*mesh);
			ctx.paint();

			Ok(())
		})
	}

	fn history_len(&self) -> usize {
		1
	}
}