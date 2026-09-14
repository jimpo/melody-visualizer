use palette::{FromColor, Hsv, RgbHue, encoding::Srgb, rgb::Rgb};
use std::any::Any;
use std::cmp;
use std::collections::VecDeque;
use std::f64::consts::{PI, TAU};
use std::fmt;
use std::iter;
use std::sync::Arc;

use crate::error::Error;
use crate::graphic::{Graphic, GraphicBuffer, GraphicGenerator};
use crate::spectrum::{LogHz, Spectrum, SpectrumParams};
use crate::traits::Configurable;

pub struct SpiralGenerator {
	x_max: i32,
	y_max: i32,
	config: Config,
	params: Arc<SpectrumParams>,
	/// What every pixel shows, one entry a pixel in the buffer's row-major
	/// order. Rebuilt on a change of size, grid or config.
	map: Vec<Texel>,
	/// Each bin's colour at full brightness, as linear RGB with its brightest
	/// channel at 1. Rebuilt with `map`.
	hues: Vec<[f32; 3]>,
	/// Each bin's colour at the brightness of the frame being drawn, as a
	/// `0x00RRGGBB` word in sRGB. `generate` refills it; it lives here so that a
	/// frame allocates nothing.
	lit: Vec<u32>,
	/// Whether `map` and `hues` are out of date. A rebuild is a pass over every
	/// pixel, so the hooks only set this and the next frame rebuilds: a slider
	/// drag that reconfigures the generator many times a frame costs one
	/// rebuild a frame.
	stale: bool,
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
	/// Whether to name each spoke by its interval from the key, in a ring
	/// outside the outermost turn. The ring takes [`RING_ROOM`] pixels on top of
	/// `outer_pad`.
	#[serde(default)]
	pub interval_ring: bool,
	/// The gain on a bin's value, as light. Above 1 a bin short of the peak
	/// lights fully, and a bin at or over it whitens.
	#[serde(default = "default_exposure")]
	pub exposure: f64,
	/// The half-width of the glow's flat core of full brightness, in octaves.
	#[serde(default = "default_core")]
	pub core: f64,
	/// The sigma of the Gaussian the glow falls off in past the core's edge, in
	/// octaves.
	///
	/// The turns are an octave apart, so a glow whose `core + REACH · sigma`
	/// passes half an octave is cut off where it meets the next turn's.
	#[serde(default = "default_sigma")]
	pub sigma: f64,
}

/// A bin at 80% of the running peak reaches full brightness.
pub const DEFAULT_EXPOSURE: f64 = 1.25;
/// About a quarter of a semitone either side of the centre line.
pub const DEFAULT_CORE: f64 = 0.02;
/// Keeps the ribbon about as wide as a Gaussian glow alone of 0.05 octaves.
pub const DEFAULT_SIGMA: f64 = 0.04;

fn default_exposure() -> f64 {
	DEFAULT_EXPOSURE
}

fn default_core() -> f64 {
	DEFAULT_CORE
}

fn default_sigma() -> f64 {
	DEFAULT_SIGMA
}

/// The interval names of the twelve spokes, clockwise from the key.
const INTERVALS: [&str; 12] = [
	"P1", "m2", "M2", "m3", "M3", "P4", "TT", "P5", "m6", "M6", "m7", "M7",
];
/// The pixels the interval ring takes from the spiral's radius. The labels
/// reach about 8 px further, into `outer_pad`, so a padding under that clips
/// them.
pub const RING_ROOM: f64 = 23.0;
/// Where a label's centre sits, in pixels outside the outermost turn.
const LABEL_OFFSET: f64 = 20.0;
/// Cairo matches the face through fontconfig, which falls back to its default
/// face (DejaVu Sans on a stock Ubuntu) when it is not installed.
// ponytail: cairo's toy text API offers only normal and bold, not the design's
// weight 500; move to pangocairo if the weight matters.
const LABEL_FACE: &str = "Playfair Display";
const LABEL_SIZE: f64 = 13.0;
const LABEL_ALPHA: f64 = 0.55;

/// The glow's brightness maths runs in linear light and is encoded to sRGB
/// with this power law. For a power law, encoding distributes over
/// multiplication, so `dim` multiplying two encoded bytes multiplies their light.
/// The piecewise sRGB curve would break that, and the difference does not show.
const GAMMA: f64 = 2.2;
/// Where the glow ends, in sigmas past the core's edge. The encoded weight is
/// `255 · exp(−s²/2γ)`, which rounds to zero past `√(2γ · ln 510)`, about 5.24,
/// so ending it here leaves no step at its edge.
pub const REACH: f64 = 5.25;
/// A silent bin's brightness, as a fraction of full light.
const REST: f64 = 0.02;

/// What one pixel of the surface shows: which bin lights it and how much.
///
/// A weight of zero is black whatever the bin, so a pixel off the ribbon needs
/// no sentinel.
#[derive(Clone, Copy)]
struct Texel {
	/// Index into the spectrum's values.
	bin: u16,
	/// Glow, sRGB-encoded: 255 inside the ribbon's core and falling to 0 with
	/// distance past its edge.
	weight: u8,
}

impl Configurable for SpiralGenerator {
	type Config = Config;

	fn new(config: Config) -> Self {
		SpiralGenerator {
			x_max: 0,
			y_max: 0,
			config,
			params: Arc::new(SpectrumParams::default()),
			map: Vec::new(),
			hues: Vec::new(),
			lit: Vec::new(),
			stale: false,
		}
	}

	fn set_config(&mut self, config: Config) {
		// `generate` reads the exposure every frame, so a change to it alone
		// leaves the map current.
		self.stale |= Config {
			exposure: self.config.exposure,
			..config.clone()
		} != self.config;
		self.config = config;
	}
}

impl fmt::Debug for SpiralGenerator {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		// The map has an entry per pixel, far too many to print.
		f.debug_struct("SpiralGenerator")
			.field("x_max", &self.x_max)
			.field("y_max", &self.y_max)
			.field("config", &self.config)
			.finish_non_exhaustive()
	}
}

impl SpiralGenerator {
	/// Rebuilds `hues` and `map` for the current size, grid and config.
	///
	/// The spiral places a bin of pitch `L` (in log₂ Hz) at radius
	/// `r_min + r_scale · (L − min)`, a turn per octave clockwise from straight
	/// up, with the key at the top. This inverts that per pixel: it finds the
	/// nearest point of the ribbon, which bin lies there, and how far off the
	/// pixel is.
	fn regenerate(&mut self) {
		self.hues.clear();
		self.map.clear();

		let log_frequencies = self.params.log_frequencies();
		let (Some(&min), Some(&max)) = (log_frequencies.first(), log_frequencies.last()) else {
			return;
		};
		let bins = log_frequencies.len();
		assert!(
			bins <= usize::from(u16::MAX) + 1,
			"a texel holds its bin as a u16"
		);
		let Config {
			key_log_freq: key,
			core,
			sigma,
			..
		} = self.config;

		self.hues.extend(log_frequencies.iter().map(|&log_freq| {
			let turn = (log_freq - key).rem_euclid(1.0);
			let hsv = <Hsv<Srgb, f64>>::new(RgbHue::from_radians(turn * TAU), 0.8, 1.0);
			let rgb = Rgb::<Srgb, f64>::from_color(hsv);
			[rgb.red, rgb.green, rgb.blue].map(|channel| channel.powf(GAMMA) as f32)
		}));

		let r_min = self.config.center_pad;
		let r_scale = (self.r_max() - r_min) / (max - min);
		let (x_origin, y_origin) = self.origin();
		let (width, height) = (self.x_max, self.y_max);

		// ponytail: about 30 ms at 1280x800 and 60 ms at 1920x1080, longer than a
		// spectrum tick, so frames drop and the spectrum thread skips ticks while
		// the window resizes or a slider drags. Skip the pixels outside the
		// annulus, or solve in f32, if that shows.
		let pixels = (0..height).flat_map(|y| (0..width).map(move |x| (x, y)));
		self.map.extend(pixels.map(|(x, y)| {
			// The pixel centre relative to the origin, y pointing up.
			let dx = x as f64 + 0.5 - x_origin;
			let dy = y_origin - (y as f64 + 0.5);
			// Clockwise from straight up, as a fraction of a turn.
			let turn = (dx.atan2(dy) / TAU).rem_euclid(1.0);

			// The ribbon crosses this angle once an octave, at every pitch
			// `key + turn + k` for a whole number `k`, and those crossings are
			// `r_scale` apart. `pitch` is what this radius would carry on the
			// centre line, relative to the key, so the nearest crossing is the `k`
			// it rounds to. A pixel midway between two turns is past the glow of
			// both and stays black.
			let pitch = ((dx * dx + dy * dy).sqrt() - r_min) / r_scale + min - key;
			let k = (pitch - turn).round();
			// Distance past the core's edge, in sigmas. Past the glow's end is off
			// the ribbon, which is most of the surface, so `exp` is skipped there.
			let sigmas = (((pitch - turn - k).abs() - core) / sigma).max(0.0);

			let bin = ((key + turn + k - min) / (max - min) * (bins - 1) as f64).round();
			if sigmas < REACH && (0.0..bins as f64).contains(&bin) {
				Texel {
					bin: bin as u16,
					// The Gaussian in light, encoded: `exp(x)^(1/γ)` is `exp(x/γ)`.
					weight: (255.0 * (-0.5 * sigmas * sigmas / GAMMA).exp()).round() as u8,
				}
			} else {
				// Off the ribbon: too far from its centre line, or beyond either end.
				Texel { bin: 0, weight: 0 }
			}
		}));
	}

	/// The radius of the outermost turn's centre line.
	fn r_max(&self) -> f64 {
		let ring = if self.config.interval_ring {
			RING_ROOM
		} else {
			0.0
		};
		(cmp::min(self.x_max, self.y_max) as f64 / 2.0 - self.config.outer_pad - ring)
			.max(self.config.center_pad)
	}

	fn origin(&self) -> (f64, f64) {
		(self.x_max as f64 / 2.0, self.y_max as f64 / 2.0)
	}

	/// Names each spoke by its interval from the key, in upright labels outside
	/// the outermost turn.
	fn draw_interval_ring(&self, ctx: &cairo::Context) -> Result<(), cairo::Error> {
		let r = self.r_max() + LABEL_OFFSET;
		let (x_origin, y_origin) = self.origin();

		ctx.set_source_rgba(1.0, 1.0, 1.0, LABEL_ALPHA);
		ctx.select_font_face(
			LABEL_FACE,
			cairo::FontSlant::Normal,
			cairo::FontWeight::Normal,
		);
		ctx.set_font_size(LABEL_SIZE);
		let font = ctx.font_extents()?;
		for (step, name) in INTERVALS.iter().enumerate() {
			let (sin, cos) = (step as f64 / 12.0 * 2.0 * PI).sin_cos();
			let (x, y) = (x_origin + sin * r, y_origin - cos * r);
			let advance = ctx.text_extents(name)?.x_advance();
			// Centred on the advance and the em box rather than on the ink, so
			// the old-style figures that drop below the baseline keep their drop.
			ctx.move_to(
				x - advance / 2.0,
				y + (font.ascent() - font.descent()) / 2.0,
			);
			ctx.show_text(name)?;
		}
		Ok(())
	}
}

/// `hue`, in linear RGB with its brightest channel at 1, lit to `light`, as a
/// `0x00RRGGBB` word in sRGB.
///
/// Up to 1 the hue scales with `light`. Past 1 its brightest channel is full,
/// and the colour moves in light toward white, reaching it at 2, the way an
/// overexposed lamp blows out.
fn expose(hue: [f32; 3], light: f32) -> u32 {
	let over = (light - 1.0).clamp(0.0, 1.0);
	let [red, green, blue] = hue.map(|channel| {
		let linear = channel * light.min(1.0) + (1.0 - channel) * over;
		(255.0 * linear.powf(1.0 / GAMMA as f32)).round() as u32
	});
	(red << 16) | (green << 8) | blue
}

/// `colour`, a `0x00RRGGBB` word, with every channel scaled by `weight / 255`.
///
/// Multiplying by `weight + 1` and dividing by 256 keeps both ends exact, black
/// at 0 and `colour` itself at 255, at the price of a multiply and a shift.
fn dim(colour: u32, weight: u8) -> u32 {
	let factor = u32::from(weight) + 1;
	let channel = |shift: u32| ((((colour >> shift) & 0xff) * factor) >> 8) << shift;
	channel(16) | channel(8) | channel(0)
}

impl GraphicGenerator for SpiralGenerator {
	fn generate(
		&mut self,
		mut buffer: GraphicBuffer,
		spectrum_history: &VecDeque<Spectrum>,
	) -> Result<Graphic, Error> {
		if self.stale {
			self.regenerate();
			self.stale = false;
		}

		let values = spectrum_history.front().map(|spectrum| spectrum.values());

		self.lit.clear();
		self.lit
			.extend(self.hues.iter().enumerate().map(|(bin, &hue)| {
				let value = values.map_or(0.0, |values| values[bin]);
				let light = REST + (1.0 - REST) * self.config.exposure * value;
				expose(hue, light as f32)
			}));

		let pixels = buffer.data_mut();
		if self.map.is_empty() {
			// No grid yet, so nothing lights the surface.
			pixels.fill(0);
		} else {
			assert_eq!(
				pixels.len(),
				4 * self.map.len(),
				"the map is built for the size of the buffer",
			);
			for (pixel, texel) in iter::zip(pixels.as_chunks_mut::<4>().0, &self.map) {
				// Most of the surface is off the ribbon, and black needs no lookup.
				let colour = if texel.weight == 0 {
					0
				} else {
					dim(self.lit[usize::from(texel.bin)], texel.weight)
				};
				*pixel = colour.to_ne_bytes();
			}
		}

		if self.config.interval_ring {
			buffer.draw(|ctx| Ok(self.draw_interval_ring(ctx)?))
		} else {
			Ok(Graphic { buffer })
		}
	}

	fn set_params(&mut self, params: &Arc<SpectrumParams>) {
		self.params = params.clone();
		self.stale = true;
	}

	fn set_size(&mut self, width: i32, height: i32) {
		self.x_max = width;
		self.y_max = height;
		self.stale = true;
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
	/// The distance between two turns of the ribbon.
	const OCTAVE: f64 = (R_MAX - CENTER_PAD) / OCTAVES as f64;

	fn config() -> Config {
		Config {
			outer_pad: OUTER_PAD,
			center_pad: CENTER_PAD,
			key_log_freq: note!(C, 4).log_frequency(),
			interval_ring: false,
			exposure: 1.0,
			core: DEFAULT_CORE,
			sigma: DEFAULT_SIGMA,
		}
	}

	/// How far the glow reaches from the centre line, in pixels.
	fn reach() -> f64 {
		(config().core + REACH * config().sigma) * OCTAVE
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
		refresh(&mut generator);
		generator
	}

	/// Draws a frame with no history, which rebuilds whatever the hooks left
	/// stale.
	fn refresh(generator: &mut SpiralGenerator) {
		let buffer = GraphicBuffer::new(generator.x_max, generator.y_max);
		generator
			.generate(buffer, &VecDeque::new())
			.expect("rendering onto a CPU surface needs no display");
	}

	/// `generator`'s frame for a spectrum whose every bin holds `value`.
	fn render(generator: &mut SpiralGenerator, value: f64) -> Graphic {
		let spectrum = SpectrumBuffer::new(semitone_grid()).fill(|data, _params| data.fill(value));
		let history = VecDeque::from([spectrum]);
		generator
			.generate(GraphicBuffer::new(SIZE, SIZE), &history)
			.expect("rendering onto a CPU surface needs no display")
	}

	/// The radius of bin `index`'s centre line on [`keyed_to_c`]'s surface.
	fn radius(index: usize) -> f64 {
		CENTER_PAD + (R_MAX - CENTER_PAD) * index as f64 / (BINS - 1) as f64
	}

	/// Where bin `index`'s centre line lands on [`keyed_to_c`]'s surface: a turn
	/// an octave, clockwise from a C straight up.
	fn centre(index: usize) -> (f64, f64) {
		let theta = (index % 12) as f64 / 12.0 * TAU;
		let radius = radius(index);
		(ORIGIN + radius * theta.sin(), ORIGIN - radius * theta.cos())
	}

	/// The texel of the pixel that `(x, y)` falls in.
	fn texel_at(generator: &SpiralGenerator, (x, y): (f64, f64)) -> Texel {
		generator.map[y as usize * generator.x_max as usize + x as usize]
	}

	/// The brightest channel within a pixel of `(x, y)`.
	///
	/// The neighbourhood absorbs the rounding between a point in floating point
	/// and the pixel grid it lands on.
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
			let texel = texel_at(&generator, (ORIGIN, ORIGIN - radius(index)));
			assert_eq!(
				usize::from(texel.bin),
				index,
				"straight up at this radius is a C, bin {}",
				index,
			);
			assert!(texel.weight > 200, "and it is on the centre line");
		}
	}

	#[test]
	fn frequencies_an_octave_apart_share_an_angle_and_a_hue() {
		let generator = keyed_to_c();
		// A tritone from the key, half a turn round, so neither the angle nor the
		// hue sits on the wrap-around where two representations of the same
		// direction differ.
		let indices = [6, 18, 30];
		for index in indices {
			let texel = texel_at(&generator, (ORIGIN, ORIGIN + radius(index)));
			assert_eq!(
				usize::from(texel.bin),
				index,
				"straight down is the tritone in every octave",
			);
		}
		let [first, second, third] = indices.map(|index| generator.hues[index]);
		assert_eq!(first, second);
		assert_eq!(first, third);
	}

	#[test]
	fn radius_increases_with_log_frequency() {
		let generator = keyed_to_c();
		// Up the spoke every C sits on, from the centre to the edge.
		let bins = (0..SIZE / 2)
			.rev()
			.map(|y| generator.map[(y * SIZE + SIZE / 2) as usize])
			.filter(|texel| texel.weight > 0)
			.map(|texel| texel.bin)
			.collect::<Vec<_>>();
		assert_eq!(bins.first(), Some(&0));
		assert_eq!(bins.last(), Some(&(BINS as u16 - 1)));
		assert!(
			bins.is_sorted(),
			"radius is the log-frequency axis, so it never doubles back",
		);
	}

	#[test]
	fn the_ribbon_spans_the_configured_annulus() {
		let generator = keyed_to_c();
		// Straight up, where both ends of a spiral keyed to C sit.
		let straight_up = |radius: f64| texel_at(&generator, (ORIGIN, ORIGIN - radius));

		let first = straight_up(CENTER_PAD);
		let last = straight_up(R_MAX);
		assert_eq!(usize::from(first.bin), 0);
		assert_eq!(usize::from(last.bin), BINS - 1);
		assert!(first.weight > 200 && last.weight > 200);

		// The glow is dim two and a half sigmas past the core's edge, and gone
		// past its end.
		let dim_edge = (config().core + 2.5 * config().sigma) * OCTAVE;
		for radius in [CENTER_PAD - dim_edge, R_MAX + dim_edge] {
			assert!(straight_up(radius).weight < 128);
		}
		// Only inside the first turn: outside the last, the glow reaches the edge
		// of this small surface.
		let gone = reach() + 2.0;
		assert_eq!(straight_up(CENTER_PAD - gone).weight, 0);
	}

	#[test]
	fn the_core_is_at_full_brightness_up_to_its_edge() {
		let mut generator = keyed_to_c();
		// A core a few pixels wide, so a pixel inside its edge is off the centre
		// line.
		let core = 0.1;
		generator.set_config(Config { core, ..config() });
		refresh(&mut generator);
		// Straight up from the middle turn's C. A pixel centre lies up to half a
		// pixel from the sampled point.
		let edge = radius(12) + core * OCTAVE;
		let weight = |radius: f64| texel_at(&generator, (ORIGIN, ORIGIN - radius)).weight;
		assert_eq!(
			weight(edge - 1.0),
			255,
			"a pixel inside the core is at full brightness"
		);
		assert!(weight(edge + 1.0) < 255, "a pixel past its edge is dimmer");
	}

	#[test]
	fn geometry_is_rebuilt_on_a_size_change_and_on_a_params_change() {
		let mut generator = keyed_to_c();

		generator.set_size(SIZE / 2, SIZE / 2);
		refresh(&mut generator);
		assert_eq!(generator.map.len(), (SIZE / 2 * SIZE / 2) as usize);
		let origin = SIZE as f64 / 4.0;
		let outermost = texel_at(&generator, (origin, OUTER_PAD));
		assert_eq!(
			usize::from(outermost.bin),
			BINS - 1,
			"a smaller surface rescales the annulus to its own edge",
		);
		assert!(outermost.weight > 128);

		generator.set_params(&Arc::new(SpectrumParams::exp_spaced(7, 200.0, 3200.0)));
		refresh(&mut generator);
		assert_eq!(
			generator.hues.len(),
			7,
			"a new grid gives one colour per bin of it",
		);
		assert_eq!(
			generator.map.iter().map(|texel| texel.bin).max(),
			Some(6),
			"and the map reaches its last bin",
		);
	}

	#[test]
	fn the_disc_inside_center_pad_is_black() {
		let mut generator = keyed_to_c();
		let graphic = render(&mut generator, 1.0);

		// Inside the innermost band, which reaches the glow's end below
		// `center_pad`.
		let clear = CENTER_PAD - reach() - 2.0;
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
			let (x, y) = centre(index);
			let brightest = brightest_near(&graphic, x, y);
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

		// `generate` maps a silent bin to `REST` of full light, about a sixth of
		// full brightness once encoded, so the spiral still shows.
		for index in (1..BINS).step_by(6) {
			let (x, y) = centre(index);
			let brightest = brightest_near(&graphic, x, y);
			assert!(
				(20..90).contains(&brightest),
				"bin {} is silent, so its band is dim but visible, not {}",
				index,
				brightest,
			);
		}
	}

	/// [`keyed_to_c`] at `exposure`, having drawn a frame whose every bin holds
	/// `value`.
	fn exposed(exposure: f64, value: f64) -> SpiralGenerator {
		let mut generator = keyed_to_c();
		generator.set_config(Config {
			exposure,
			..config()
		});
		render(&mut generator, value);
		generator
	}

	/// The red, green and blue bytes of a `0x00RRGGBB` word.
	fn channels(colour: u32) -> [u8; 3] {
		[16, 8, 0].map(|shift| (colour >> shift) as u8)
	}

	#[test]
	fn a_bin_at_the_peak_at_exposure_one_is_its_hue_at_full_brightness() {
		let generator = exposed(1.0, 1.0);
		// Bin 1 is a semitone, a twelfth of a turn round the wheel: an orange
		// whose three channels all differ.
		let hsv = <Hsv<Srgb, f64>>::new(RgbHue::from_radians(TAU / 12.0), 0.8, 1.0);
		let hue = Rgb::<Srgb, f64>::from_color(hsv).into_format::<u8>();
		let lit = channels(generator.lit[1]);
		for (lit, hue) in iter::zip(lit, [hue.red, hue.green, hue.blue]) {
			// The hue goes to linear light and back, which can round a byte off.
			assert!(
				lit.abs_diff(hue) <= 1,
				"lit {lit:?} is the hue {hue:?} at full brightness",
			);
		}
	}

	#[test]
	fn a_bin_at_the_peak_part_way_to_white_keeps_its_brightest_channel_full() {
		let hue = channels(exposed(1.0, 1.0).lit[1]);
		let lit = channels(exposed(1.5, 1.0).lit[1]);
		assert_eq!(lit[0], 255, "red is the orange's brightest channel");
		for channel in 1..3 {
			assert!(
				(hue[channel] + 1..255).contains(&lit[channel]),
				"the other channels rise toward white, {hue:?} to {lit:?}",
			);
		}
	}

	#[test]
	fn a_bin_at_the_peak_at_low_exposure_is_dimmer_than_its_hue() {
		let hue = channels(exposed(1.0, 1.0).lit[1]);
		let lit = channels(exposed(0.5, 1.0).lit[1]);
		assert!(
			iter::zip(lit, hue).all(|(lit, hue)| lit < hue),
			"{lit:?} is dimmer than {hue:?}",
		);
	}

	#[test]
	fn an_exposure_change_alone_leaves_the_map_current() {
		let mut generator = keyed_to_c();
		generator.set_config(Config {
			exposure: 2.0,
			..config()
		});
		assert!(!generator.stale);
		generator.set_config(Config {
			center_pad: CENTER_PAD + 1.0,
			exposure: 3.0,
			..config()
		});
		assert!(generator.stale);
	}

	#[test]
	fn a_bin_at_the_peak_at_high_exposure_blows_out_to_white() {
		let generator = exposed(4.0, 1.0);
		for (bin, &colour) in generator.lit.iter().enumerate() {
			let lit = channels(colour);
			assert!(
				lit.iter().all(|&channel| channel > 240),
				"bin {bin} is near white, not {lit:?}",
			);
		}
	}

	#[test]
	fn a_silent_bin_rests_at_the_same_brightness_at_any_exposure() {
		let resting = exposed(1.0, 0.0).lit;
		for exposure in [0.5, 4.0] {
			assert_eq!(
				exposed(exposure, 0.0).lit,
				resting,
				"exposure scales the value, not the resting level",
			);
		}
	}

	#[test]
	fn a_config_change_is_drawn_by_the_next_frame() {
		let mut generator = keyed_to_c();
		// Half an octave up puts the key on the tritone, which turns every C to
		// the bottom of the wheel.
		generator.set_config(Config {
			key_log_freq: config().key_log_freq + 0.5,
			..config()
		});
		refresh(&mut generator);
		let texel = texel_at(&generator, (ORIGIN, ORIGIN + radius(12)));
		assert_eq!(
			usize::from(texel.bin),
			12,
			"the map is rebuilt for the new key before the frame that draws it",
		);
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

	#[test]
	fn the_interval_ring_makes_room_and_labels_every_spoke() {
		let mut generator = SpiralGenerator::new(Config {
			interval_ring: true,
			..config()
		});
		generator.set_size(SIZE, SIZE);
		generator.set_params(&semitone_grid());

		let r_max = R_MAX - RING_ROOM;
		let graphic = render(&mut generator, 0.0);
		let outermost = texel_at(&generator, (ORIGIN, ORIGIN - r_max));
		assert_eq!(
			usize::from(outermost.bin),
			BINS - 1,
			"the outermost turn moves in to make room for the ring",
		);
		assert!(outermost.weight > 200);

		for step in 0..12 {
			let theta = step as f64 / 12.0 * 2.0 * PI;
			let r = r_max + LABEL_OFFSET;
			let x = (ORIGIN + r * theta.sin()).round() as i32;
			let y = (ORIGIN - r * theta.cos()).round() as i32;
			// The box a 13 px label fills, which lies clear of the outermost band.
			let lit = (y - 6..=y + 6)
				.flat_map(|y| (x - 8..=x + 8).map(move |x| (x, y)))
				.any(|(x, y)| {
					let (red, green, blue) = graphic.pixel(x, y);
					red.max(green).max(blue) > 50
				});
			assert!(lit, "spoke {} has a label outside the outermost turn", step);
		}
	}
}
