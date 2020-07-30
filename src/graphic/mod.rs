pub mod renderer;
pub mod generators;

use cairo::{BorrowError, Context, Format, ImageSurface, Surface};
use std::{
	any::Any,
	collections::VecDeque,
	fmt::Debug,
	mem,
	sync::Arc,
};

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

	pub fn with_image_surface<T, F>(&mut self, f: F) -> Result<T, Error>
		where F: Fn(&Surface) -> Result<T, Error>
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
		let stride = Format::Rgb24.stride_for_width(width as u32)
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

	pub fn draw(mut self, draw: impl Fn(&Context) -> Result<(), Error>)
		-> Result<Graphic, Error>
	{
		self.with_image_surface(|surface| draw(&Context::new(surface)))?;
		Ok(Graphic { buffer: self })
	}

	/// The callback must destroy any copies it makes of the surface reference, even if Cairo
	/// creates the copies internally. Otherwise, this returns Error::GraphicDrawCopiesSurface.
	fn with_image_surface<T, F>(&mut self, f: F) -> Result<T, Error>
		where F: Fn(&Surface) -> Result<T, Error>
	{
		// Use unsafe cast to extend lifetime of the data reference because
		// ImageSurface::create_for_data takes ownership of the data
		// (ie. requires 'static lifetime). This is safe in here because we will ensure that all
		// references to the created ImageSurface are dropped before returning, leaving no other
		// references to the data vector.
		//
		// See https://github.com/gtk-rs/cairo/issues/335 for rationale.
		let data_ref = unsafe {
			mem::transmute::<&'_ mut [u8], &'static mut [u8]>(self.data.as_mut())
		};
		let mut surface = ImageSurface::create_for_data(
			data_ref,
			Format::Rgb24,
			self.width,
			self.height,
			self.stride
		)?;

		let result = f(&*surface);

		// ImageSurface::get_data checks that there are no additional references and the data
		// is safe to modify. If there is an error, we clone the data to avoid corruption.
		let _ = surface.get_data()
			.map_err(|err| {
				self.data = self.data.clone();
				match err {
					BorrowError::Cairo(err) => err.into(),
					BorrowError::NonExclusive => Error::GraphicDrawClonesSurface,
				}
			})?;

		result
	}
}

pub trait GraphicGenerator: Debug + Send {
	fn generate(
		&mut self,
		buffer: GraphicBuffer,
		params: &Arc<SpectrumParams>,
		spectrum_history: &VecDeque<Spectrum>,
	) -> Result<Graphic, Error>;

	fn history_len(&self) -> usize;

	fn upcast_any_ref(&self) -> &dyn Any;
	fn upcast_any_mut(&mut self) -> &mut dyn Any;
}
