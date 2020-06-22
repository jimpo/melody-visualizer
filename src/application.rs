use cairo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceType {
	Audio,
	MIDI,
}

pub struct Controller {
	source_type: SourceType,
	graphic: Option<cairo::ImageSurface>,
}

impl Controller {
	pub fn new() -> Self {
		let source_type = SourceType::Audio;

		Controller {
			source_type,
			graphic: None,
		}
	}

	pub fn graphic_mut(&mut self) -> &mut Option<cairo::ImageSurface> {
		&mut self.graphic
	}

	pub fn on_visualization_resize(x_max: i32, y_max: i32) {}
}