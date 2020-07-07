#[derive(Clone)]
pub struct SpectrumBuffer {
}

impl SpectrumBuffer {

}

#[derive(Clone)]
pub struct Spectrum {
}

impl Spectrum {
	pub fn into_buffer(self) -> SpectrumBuffer {
		SpectrumBuffer {}
	}
}