use futures::{prelude::*, future::Either};
use std::{
	cell::RefCell,
	rc::Rc,
};

use crate::app::config::SpectrumTransformConfig;
use crate::async_processor::AsyncProcessor;
use crate::controllers::app::AppController;
use crate::error::Error;
use crate::spectrum::renderer::SpectrumRenderer;

pub struct DiffuserController {
	id: u64,
	app_controller: Rc<RefCell<AppController>>,
	spectrum_renderer: AsyncProcessor<SpectrumRenderer>,
}

impl DiffuserController {
	pub fn new(id: u64, app_controller: Rc<RefCell<AppController>>) -> Rc<RefCell<Self>> {
		let spectrum_renderer = app_controller.borrow().spectrum_renderer().clone();
		let controller = Rc::new(RefCell::new(DiffuserController {
			id,
			app_controller,
			spectrum_renderer,
		}));
		controller
	}

	pub fn update_width(&mut self, width: f64) -> impl Future<Output=Result<(), Error>> {
		match self.set_width(width) {
			Ok(()) => Either::Left(
				self.app_controller.borrow()
					.update_spectrum_transform(self.id)
			),
			Err(err) => Either::Right(future::err(err)),
		}
	}

	fn set_width(&mut self, width: f64) -> Result<(), Error> {
		let mut app_controller = self.app_controller.borrow_mut();
		let config = app_controller.config.spectrum_transforms.get_mut(&self.id)
			.ok_or_else(|| Error::MissingTransform { id: self.id })?;
		match config {
			SpectrumTransformConfig::Diffuser(config) => {
				config.width = width;
				Ok(())
			}
			config => Err(Error::UnexpectedConfigEntry(format!(
				"spiral control signal fired when other graphic generator is configured: {:?}",
				config
			))),
		}
	}
}