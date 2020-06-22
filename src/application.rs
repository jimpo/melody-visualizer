use cairo;
use log::debug;

#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
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

	pub fn get_source_type(&self) -> SourceType {
		self.source_type
	}

	pub fn set_source_type(&mut self, source_type: SourceType) {
		if source_type == self.source_type {
			return;
		}
		debug!("Source type \"{}\" activated", source_type);

		self.source_type = source_type;
	}
}