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

	// Sadly, this is mutable because a Surface reference can be used to modify its backing data,
	// which we do not want to make a copy of for performance reasons. It is recommended that the
	// callback only use the Surface argument in an immutable way.
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

	pub fn draw(mut self, draw: impl Fn(&Context) -> Result<(), Error>) -> Result<Graphic, Error> {
		self.with_image_surface(|surface| draw(&Context::new(surface)?))?;
		Ok(Graphic { buffer: self })
	}

	/// The callback must destroy any copies it makes of the surface reference, even if Cairo
	/// creates the copies internally. Otherwise, this returns Error::GraphicDrawCopiesSurface.
	fn with_image_surface<T, F>(&mut self, f: F) -> Result<T, Error>
	where
		F: Fn(&Surface) -> Result<T, Error>,
	{
		// Use unsafe cast to extend lifetime of the data reference because
		// ImageSurface::create_for_data takes ownership of the data
		// (ie. requires 'static lifetime). This is safe in here because we will ensure that all
		// references to the created ImageSurface are dropped before returning, leaving no other
		// references to the data vector.
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

		// ImageSurface::get_data checks that there are no additional references and the data
		// is safe to modify. If there is an error, we clone the data to avoid corruption.
		let _ = surface.data().map_err(|err| {
			self.data = self.data.clone();
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
