pub mod generators;
pub mod renderer;

use std::{any::Any, collections::VecDeque, fmt::Debug, sync::Arc};

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

	/// The frame's width, height and RGBA pixels, row-major.
	pub fn into_rgba(self) -> (u32, u32, Vec<u8>) {
		let GraphicBuffer {
			width,
			height,
			data,
		} = self.buffer;
		(width as u32, height as u32, data)
	}

	/// The pixel at `(x, y)` as `(red, green, blue)`.
	///
	/// # Preconditions
	/// - `(x, y)` is inside the graphic.
	#[cfg(test)]
	fn pixel(&self, x: i32, y: i32) -> (u8, u8, u8) {
		self.buffer.pixel(x, y)
	}
}

#[derive(Clone, Default)]
pub struct GraphicBuffer {
	width: i32,
	height: i32,
	/// RGBA, one byte a channel, row-major with no padding between rows: the
	/// layout a GPU texture upload takes as it is.
	data: Vec<u8>,
}

impl GraphicBuffer {
	pub fn new(width: i32, height: i32) -> Self {
		GraphicBuffer::default().resize(width, height)
	}

	pub fn width(&self) -> i32 {
		self.width
	}

	pub fn height(&self) -> i32 {
		self.height
	}

	pub fn resize(self, width: i32, height: i32) -> Self {
		let mut data = self.data;
		data.resize(4 * (width * height) as usize, 0);
		GraphicBuffer {
			width,
			height,
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
		let offset = 4 * (y * self.width + x) as usize;
		let [red, green, blue, _] = self.data[offset..offset + 4] else {
			unreachable!("a range of four is four long")
		};
		(red, green, blue)
	}

	/// The pixels, as RGBA bytes in row-major order.
	pub fn data_mut(&mut self) -> &mut [u8] {
		&mut self.data
	}

	/// The finished frame.
	pub fn into_graphic(self) -> Graphic {
		Graphic { buffer: self }
	}
}

/// Draws a spectrum history into a pixel buffer.
///
/// A generator derives its geometry from the frequency grid and the surface
/// size. Both arrive through their own hook, so [`generate`](Self::generate)
/// never compares them with what it drew last.
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

	/// Hands the generator the frequency grid, to rebuild whatever it derives
	/// from it now or before the next frame.
	///
	/// Called when the grid changes, and once when the generator joins a
	/// renderer. The default does nothing, which is right for a generator whose
	/// output depends only on the values it is handed.
	fn set_params(&mut self, _params: &Arc<SpectrumParams>) {}

	/// Hands the generator the surface size, to rebuild whatever it derives
	/// from it now or before the next frame.
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

	#[test]
	fn the_geometry_holds_across_widths() {
		for width in [1, 2, 3, 5, 7, 13, 101] {
			let height = 3;
			let buffer = GraphicBuffer::new(width, height);

			assert_eq!(
				buffer.data.len(),
				(4 * width * height) as usize,
				"four bytes a pixel, with no padding between rows",
			);
		}
	}

	#[test]
	fn a_pixel_is_addressed_row_major() {
		let (width, height) = (13, 3);
		let mut buffer = GraphicBuffer::new(width, height);
		let last = 4 * (width * height - 1) as usize;
		buffer.data_mut()[last..].copy_from_slice(&[255, 128, 1, 255]);
		let graphic = buffer.into_graphic();

		assert_eq!(graphic.pixel(width - 1, height - 1), (255, 128, 1));
		assert_eq!(graphic.pixel(width - 2, height - 1), (0, 0, 0));
		assert_eq!(graphic.pixel(width - 1, height - 2), (0, 0, 0));
	}

	#[test]
	fn resize_sets_the_length_and_keeps_the_allocation_when_it_can() {
		let buffer = GraphicBuffer::new(64, 64);
		let allocation = buffer.data.as_ptr();

		let smaller = buffer.resize(32, 32);
		assert_eq!(smaller.data.len(), 4 * 32 * 32);
		assert_eq!(
			smaller.data.as_ptr(),
			allocation,
			"shrinking reuses the allocation, which is what makes a window resize cheap",
		);

		let larger = smaller.resize(128, 128);
		assert_eq!(larger.data.len(), 4 * 128 * 128);
	}

	#[test]
	fn a_zero_by_zero_buffer_is_a_frame() {
		// The state every buffer starts in, before the window reports a size.
		let graphic = GraphicBuffer::new(0, 0).into_graphic();
		assert_eq!((graphic.width(), graphic.height()), (0, 0));
	}
}
