#[derive(Clone, Default)]
pub struct SpectrumBuffer {
}

impl SpectrumBuffer {

}

#[derive(Clone, Default)]
pub struct Spectrum {
}

impl Spectrum {
	pub fn into_buffer(self) -> SpectrumBuffer {
		SpectrumBuffer {}
	}
}