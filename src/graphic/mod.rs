pub mod generators;
pub mod renderer;

use cairo::{BorrowError, Context, Format, ImageSurface, Surface};
use std::{any::Any, collections::VecDeque, fmt::Debug, mem, sync::Arc};

use crate::error::Error;
use crate::spectrum::{Spectrum, SpectrumParams};

#[derive(Clone, Default)]
pub struct Graphic {
	buffer: GraphicBuffer,
}

impl Graphic {
	pub fn into_buffer(self) -> GraphicBuffer {
		self.buffer
	}

	pub fn width(&self) -> i32 {
		self.buffer.width()
	}

	pub fn height(&self) -> i32 {
		self.buffer.height()
	}

	/// The pixel at `(x, y)` as `(red, green, blue)`.
	///
	/// # Preconditions
	/// - `(x, y)` is inside the graphic.
	#[cfg(test)]
	fn pixel(&self, x: i32, y: i32) -> (u8, u8, u8) {
		self.buffer.pixel(x, y)
	}

	/// Runs `f` against a cairo surface over this graphic's pixels.
	///
	/// The blit in `gui/visualization.rs` is what this exists for: cairo needs a
	/// `Surface` to copy from, and the pixels must not be copied to produce one.
	///
	/// # Preconditions
	/// - `f` must not let any clone of the surface — including one cairo makes
	///   internally, such as the reference a `Context` holds after
	///   `set_source_surface` — outlive the call. See
	///   [`GraphicBuffer::with_image_surface`] for what happens if it does.
	///
	/// The receiver is `&mut self` because a surface can write through to the
	/// pixels behind it. `f` takes a shared reference and is expected not to.
	pub fn with_image_surface<T, F>(&mut self, f: F) -> Result<T, Error>
	where
		F: Fn(&Surface) -> Result<T, Error>,
	{
		self.buffer.with_image_surface(f)
	}
}

#[derive(Clone, Default)]
pub struct GraphicBuffer {
	width: i32,
	height: i32,
	stride: i32,
	data: Vec<u8>,
}

impl GraphicBuffer {
	pub fn new(width: i32, height: i32) -> Self {
		let buffer = GraphicBuffer {
			width: 0,
			height: 0,
			stride: 0,
			data: Vec::new(),
		};
		buffer.resize(width, height)
	}

	pub fn width(&self) -> i32 {
		self.width
	}

	pub fn height(&self) -> i32 {
		self.height
	}

	pub fn resize(self, width: i32, height: i32) -> Self {
		let stride = Format::Rgb24
			.stride_for_width(width as u32)
			.expect("stride_for_width cannot fail");

		let mut data = self.data;
		data.resize((stride * height) as usize, 0);

		GraphicBuffer {
			width,
			height,
			stride,
			data,
		}
	}

	/// The pixel at `(x, y)` as `(red, green, blue)`.
	///
	/// # Preconditions
	/// - `(x, y)` is inside the buffer.
	#[cfg(test)]
	fn pixel(&self, x: i32, y: i32) -> (u8, u8, u8) {
		assert!((0..self.width).contains(&x) && (0..self.height).contains(&y));
		// Rgb24 is one 32-bit native-endian word per pixel, upper byte unused.
		let offset = (y * self.stride + x * 4) as usize;
		let word = u32::from_ne_bytes(
			self.data[offset..offset + 4]
				.try_into()
				.expect("the slice is four bytes long"),
		);
		((word >> 16) as u8, (word >> 8) as u8, word as u8)
	}

	pub fn draw(mut self, draw: impl Fn(&Context) -> Result<(), Error>) -> Result<Graphic, Error> {
		self.with_image_surface(|surface| draw(&Context::new(surface)?))?;
		Ok(Graphic { buffer: self })
	}

	/// Runs `f` against a transient cairo surface over `self.data`.
	///
	/// # Preconditions
	/// - `f` must not let any clone of the surface outlive the call, cairo's own
	///   internal references included. A `Context` holds one after
	///   `set_source_surface`, and releases it when the source is replaced.
	///
	/// A violation is detected, not prevented: it returns
	/// [`Error::GraphicDrawClonesSurface`] and leaks this buffer's allocation.
	///
	/// # Why the surface is transient
	///
	/// [`Graphic`] must be `Send`: it crosses from the graphic thread to the GTK
	/// thread over the [`AsyncProcessor`](crate::async_processor::AsyncProcessor)
	/// reply channel, and `cairo::ImageSurface` is not `Send`. So the pixels live
	/// in a plain `Vec<u8>` that travels, and whichever thread needs a surface
	/// builds one around it for the length of one call. Holding an `ImageSurface`
	/// in the buffer instead — the obvious simplification, and the one that would
	/// retire the `unsafe` below — does not compile for that reason.
	fn with_image_surface<T, F>(&mut self, f: F) -> Result<T, Error>
	where
		F: Fn(&Surface) -> Result<T, Error>,
	{
		// `ImageSurface::create_for_data` boxes the data it is given and keeps it
		// as long as the surface lives, so it demands `'static`. Extend the
		// borrow to satisfy it. The surface is destroyed before this returns, and
		// the boxed value is a reference, so dropping it frees nothing — the
		// `Vec` stays the sole owner of the allocation.
		//
		// See https://github.com/gtk-rs/cairo/issues/335 for rationale.
		let data_ref =
			unsafe { mem::transmute::<&'_ mut [u8], &'static mut [u8]>(self.data.as_mut()) };
		let mut surface = ImageSurface::create_for_data(
			data_ref,
			Format::Rgb24,
			self.width,
			self.height,
			self.stride,
		)?;

		let result = f(&surface);

		// `ImageSurface::data` fails with `NonExclusive` when the surface's
		// reference count is above one, which is exactly the case the
		// precondition rules out: a live clone still points into `self.data`.
		let _ = surface.data().map_err(|err| {
			// Move the pixels to a fresh allocation and leak the old one. Freeing
			// it would leave the escaped surface reading freed memory, turning a
			// contained aliasing bug into a use-after-free; a leak on a path that
			// should never execute is the bounded outcome.
			let copy = self.data.clone();
			mem::forget(mem::replace(&mut self.data, copy));
			match err {
				BorrowError::Cairo(err) => err.into(),
				BorrowError::NonExclusive => Error::GraphicDrawClonesSurface,
			}
		})?;

		result
	}
}

/// Draws a spectrum history into a pixel buffer.
///
/// A generator derives its geometry from the frequency grid and the surface
/// size. Both arrive through their own hook, so [`generate`](Self::generate)
/// draws and nothing else.
///
/// # Preconditions
/// - [`set_params`](Self::set_params) and [`set_size`](Self::set_size) have been
///   called with the grid and the size the buffer handed to `generate` is on.
///
/// [`GraphicRenderer`](renderer::GraphicRenderer) is what upholds those: it
/// owns the grid, sees every buffer, and calls the hooks whenever either
/// changes. A generator is not otherwise reachable.
pub trait GraphicGenerator: Debug + Send {
	/// Draws `spectrum_history`, most recent first, into `buffer`.
	fn generate(
		&mut self,
		buffer: GraphicBuffer,
		spectrum_history: &VecDeque<Spectrum>,
	) -> Result<Graphic, Error>;

	/// Rebuilds whatever the generator derives from the frequency grid.
	///
	/// Called when the grid changes, and once when the generator joins a
	/// renderer. The default does nothing, which is right for a generator whose
	/// output depends only on the values it is handed.
	fn set_params(&mut self, _params: &Arc<SpectrumParams>) {}

	/// Rebuilds whatever the generator derives from the surface size.
	///
	/// Called when the size changes, and once when the generator joins a
	/// renderer. The default does nothing, which is right for a generator that
	/// reads the size off the buffer it draws into.
	fn set_size(&mut self, _width: i32, _height: i32) {}

	/// How many spectra back the generator draws.
	fn history_len(&self) -> usize;

	fn as_any_mut(&mut self) -> &mut dyn Any;
}

#[cfg(test)]
mod tests {
	use super::*;

	use std::cell::RefCell;

	/// A buffer filled with `(red, green, blue)`, at the given size.
	fn filled(width: i32, height: i32, (red, green, blue): (f64, f64, f64)) -> Graphic {
		GraphicBuffer::new(width, height)
			.draw(|ctx| {
				ctx.set_source_rgb(red, green, blue);
				ctx.paint()?;
				Ok(())
			})
			.expect("a plain fill succeeds")
	}

	#[test]
	fn the_geometry_holds_across_widths() {
		for width in [1, 2, 3, 5, 7, 13, 101] {
			let height = 3;
			let buffer = GraphicBuffer::new(width, height);

			assert!(
				buffer.stride >= 4 * width && buffer.stride % 4 == 0,
				"Rgb24 is four bytes a pixel, on a four-byte-aligned row",
			);
			assert_eq!(buffer.data.len(), (buffer.stride * height) as usize);
		}
	}

	#[test]
	fn a_pixel_is_addressed_by_stride_not_by_width() {
		// A width whose row cairo is free to pad, and a mark in the last column:
		// indexing by `4 * width` instead of by the stride would read the wrong
		// row from the second row on.
		let (width, height) = (13, 3);
		let graphic = GraphicBuffer::new(width, height)
			.draw(|ctx| {
				ctx.set_source_rgb(0.0, 0.0, 0.0);
				ctx.paint()?;
				ctx.set_source_rgb(1.0, 1.0, 1.0);
				ctx.rectangle((width - 1) as f64, (height - 1) as f64, 1.0, 1.0);
				ctx.fill()?;
				Ok(())
			})
			.expect("the fill succeeds");

		assert_eq!(graphic.pixel(width - 1, height - 1), (255, 255, 255));
		assert_eq!(graphic.pixel(width - 2, height - 1), (0, 0, 0));
		assert_eq!(graphic.pixel(width - 1, height - 2), (0, 0, 0));
	}

	#[test]
	fn resize_sets_the_length_and_keeps_the_allocation_when_it_can() {
		let buffer = GraphicBuffer::new(64, 64);
		let allocation = buffer.data.as_ptr();

		let smaller = buffer.resize(32, 32);
		assert_eq!(smaller.data.len(), (smaller.stride * 32) as usize);
		assert_eq!(
			smaller.data.as_ptr(),
			allocation,
			"shrinking reuses the allocation, which is what makes a window resize cheap",
		);

		let larger = smaller.resize(128, 128);
		assert_eq!(larger.data.len(), (larger.stride * 128) as usize);
	}

	#[test]
	fn a_zero_by_zero_buffer_renders() {
		// The state every buffer starts in, before the drawing area reports a
		// size. cairo accepts a degenerate surface, so this is not an error path.
		let buffer = GraphicBuffer::new(0, 0);
		assert_eq!(buffer.data.len(), 0);

		let graphic = filled(0, 0, (0.0, 0.0, 0.0));
		assert_eq!((graphic.width(), graphic.height()), (0, 0));
	}

	#[test]
	fn draw_writes_the_pixels_it_is_told_to() {
		let graphic = filled(4, 4, (1.0, 0.5, 0.0));
		// Rgb24 rounds each channel to eight bits; 0.5 lands on 128.
		assert!(
			(0..4).all(|y| (0..4).all(|x| graphic.pixel(x, y) == (255, 128, 0))),
			"every pixel carries the colour the callback painted",
		);
	}

	#[test]
	fn a_surface_that_outlives_the_callback_is_caught_and_the_buffer_survives() {
		let escaped = RefCell::new(None);
		let mut buffer = GraphicBuffer::new(4, 4);

		let result = buffer.with_image_surface(|surface| {
			*escaped.borrow_mut() = Some(surface.clone());
			Ok(())
		});
		assert!(matches!(result, Err(Error::GraphicDrawClonesSurface)));

		// The buffer moved to a fresh allocation and leaked the old one, so the
		// escaped surface aliases nothing the buffer will write to. Painting
		// through it is the write that would be a use-after-free had the old
		// allocation been freed instead.
		let escaped = escaped.into_inner().expect("the callback stored a clone");
		let context = Context::new(&escaped).expect("the escaped surface is still live");
		context.set_source_rgb(1.0, 1.0, 1.0);
		context.paint().expect("painting a live surface succeeds");
		drop(context);
		drop(escaped);

		let graphic = buffer
			.draw(|ctx| {
				ctx.set_source_rgb(0.0, 0.0, 0.0);
				ctx.paint()?;
				Ok(())
			})
			.expect("the buffer draws normally after the escape");
		assert!(
			(0..4).all(|y| (0..4).all(|x| graphic.pixel(x, y) == (0, 0, 0))),
			"the buffer's pixels are its own, not the ones painted white through the escapee",
		);
	}
}
