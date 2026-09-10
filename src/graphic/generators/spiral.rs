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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
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
	use crate::spectrum::SpectrumBuffer;

	const SIZE: i32 = 300;
	const OUTER_PAD: f64 = 10.0;
	const CENTER_PAD: f64 = 30.0;
	const OCTAVES: usize = 3;
	const BINS: usize = 12 * OCTAVES + 1;

	/// The origin of the spiral on a `SIZE` by `SIZE` surface.
	const ORIGIN: f64 = SIZE as f64 / 2.0;
	/// The radius of the outermost ring, from `regenerate`'s `r_max`.
	const R_MAX: f64 = ORIGIN - OUTER_PAD;

	fn config() -> Config {
		Config {
			outer_pad: OUTER_PAD,
			center_pad: CENTER_PAD,
			key_log_freq: note!(C, 4).log_frequency(),
		}
	}

	/// A semitone grid spanning `OCTAVES` octaves upward from C3.
	///
	/// Twelve bins an octave puts a bin on every semitone, so every twelfth is
	/// the pitch class the key names.
	fn semitone_grid() -> Arc<SpectrumParams> {
		Arc::new(SpectrumParams::exp_spaced(
			BINS,
			note!(C, 3).frequency(),
			note!(C, 3 + OCTAVES as i8).frequency(),
		))
	}

	/// A generator on [`semitone_grid`] at `SIZE` by `SIZE`, keyed to C.
	fn keyed_to_c() -> SpiralGenerator {
		let mut generator = SpiralGenerator::new(config());
		generator.set_size(SIZE, SIZE);
		generator.set_params(&semitone_grid());
		generator
	}

	/// `generator`'s frame for a spectrum whose every bin holds `value`.
	fn render(generator: &mut SpiralGenerator, value: f64) -> Graphic {
		let spectrum = SpectrumBuffer::new(semitone_grid()).fill(|data, _params| data.fill(value));
		let history = VecDeque::from([spectrum]);
		generator
			.generate(GraphicBuffer::new(SIZE, SIZE), &history)
			.expect("rendering onto a CPU surface needs no display")
	}

	/// The distance of `edge`'s centre line from the surface centre.
	fn radius(edge: &SegmentEdge) -> f64 {
		(edge.x_center - ORIGIN).hypot(edge.y_center - ORIGIN)
	}

	/// The angle of `edge` about the surface centre, clockwise from straight up.
	///
	/// In `(-π, π]`, so a spoke a hair either side of the origin reads as a small
	/// angle rather than one close to a full turn.
	fn angle(edge: &SegmentEdge) -> f64 {
		(edge.x_center - ORIGIN).atan2(ORIGIN - edge.y_center)
	}

	/// The brightest channel within a pixel of `(x, y)`.
	///
	/// The neighbourhood absorbs the rounding between a mesh drawn in floating
	/// point and the pixel grid it lands on.
	fn brightest_near(graphic: &Graphic, x: f64, y: f64) -> u8 {
		let (x, y) = (x.round() as i32, y.round() as i32);
		(-1..=1)
			.flat_map(|dy| (-1..=1).map(move |dx| (x + dx, y + dy)))
			.filter(|&(x, y)| (0..SIZE).contains(&x) && (0..SIZE).contains(&y))
			.map(|(x, y)| {
				let (red, green, blue) = graphic.pixel(x, y);
				red.max(green).max(blue)
			})
			.max()
			.expect("the neighbourhood of an on-surface point is not empty")
	}

	#[test]
	fn pitch_class_c_sits_at_the_angle_origin_in_every_octave() {
		let generator = keyed_to_c();
		for index in (0..BINS).step_by(12) {
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

	#[test]
	fn radius_increases_with_log_frequency() {
		let generator = keyed_to_c();
		assert!(
			generator
				.edges
				.windows(2)
				.all(|pair| radius(&pair[0]) < radius(&pair[1])),
			"radius is the log-frequency axis, so it never doubles back",
		);
	}

	#[test]
	fn the_edges_span_the_configured_annulus() {
		let generator = keyed_to_c();
		let first = generator.edges.first().expect("the grid is not empty");
		let last = generator.edges.last().expect("the grid is not empty");

		assert!((radius(first) - CENTER_PAD).abs() < 1e-6);
		assert!((radius(last) - R_MAX).abs() < 1e-6);

		// Each edge is a band of ten per cent of an octave either side of its
		// centre line.
		let thickness = (R_MAX - CENTER_PAD) / OCTAVES as f64 * 0.1;
		let inner = (last.x_inner - ORIGIN).hypot(last.y_inner - ORIGIN);
		let outer = (last.x_outer - ORIGIN).hypot(last.y_outer - ORIGIN);
		assert!((inner - (R_MAX - thickness)).abs() < 1e-6);
		assert!((outer - (R_MAX + thickness)).abs() < 1e-6);
	}

	#[test]
	fn geometry_is_rebuilt_on_a_size_change_and_on_a_params_change() {
		let mut generator = keyed_to_c();
		let outermost = radius(generator.edges.last().expect("the grid is not empty"));

		generator.set_size(SIZE / 2, SIZE / 2);
		let resized = generator.edges.last().expect("the grid is not empty");
		let origin = SIZE as f64 / 4.0;
		assert!(
			((resized.x_center - origin).hypot(resized.y_center - origin) - (origin - OUTER_PAD))
				.abs() < 1e-6,
			"a smaller surface rescales the annulus; the old radius was {}",
			outermost,
		);

		generator.set_params(&Arc::new(SpectrumParams::exp_spaced(7, 200.0, 3200.0)));
		assert_eq!(
			generator.edges.len(),
			7,
			"a new grid gives one edge per bin of it",
		);
	}

	#[test]
	fn the_disc_inside_center_pad_is_black() {
		let mut generator = keyed_to_c();
		let graphic = render(&mut generator, 1.0);

		// Inside the innermost band, which reaches a tenth of an octave below
		// `center_pad`.
		let clear = CENTER_PAD - (R_MAX - CENTER_PAD) / OCTAVES as f64 * 0.1 - 2.0;
		for step in 0..16 {
			let theta = step as f64 / 16.0 * 2.0 * PI;
			let (x, y) = (ORIGIN + clear * theta.sin(), ORIGIN - clear * theta.cos());
			assert_eq!(
				graphic.pixel(x.round() as i32, y.round() as i32),
				(0, 0, 0),
				"the centre of the spiral carries no bin",
			);
		}
		assert_eq!(graphic.pixel(SIZE / 2, SIZE / 2), (0, 0, 0));
	}

	#[test]
	fn a_bin_at_full_scale_is_bright_where_its_edge_lands() {
		let mut generator = keyed_to_c();
		let graphic = render(&mut generator, 1.0);

		for index in (1..BINS).step_by(6) {
			let edge = &generator.edges[index];
			let brightest = brightest_near(&graphic, edge.x_center, edge.y_center);
			assert!(
				brightest > 150,
				"bin {} is at full scale, so its band is near full brightness, not {}",
				index,
				brightest,
			);
		}
	}

	#[test]
	fn silence_renders_at_the_base_brightness_rather_than_black() {
		let mut generator = keyed_to_c();
		let graphic = render(&mut generator, 0.0);

		// `generate` maps a bin to `0.2 + 0.8 * value`, so a silent spectrum
		// still draws the spiral at a fifth of full brightness.
		for index in (1..BINS).step_by(6) {
			let edge = &generator.edges[index];
			let brightest = brightest_near(&graphic, edge.x_center, edge.y_center);
			assert!(
				(20..90).contains(&brightest),
				"bin {} is silent, so its band is dim but visible, not {}",
				index,
				brightest,
			);
		}
	}

	#[test]
	fn rendering_is_deterministic() {
		let mut generator = keyed_to_c();
		let first = render(&mut generator, 0.5);
		let second = render(&mut generator, 0.5);
		assert_eq!(
			first.buffer.data, second.buffer.data,
			"the same history, grid and size produce the same bytes",
		);
	}
}
