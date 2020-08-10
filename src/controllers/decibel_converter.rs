use futures::{prelude::*, future::Either};
use std::{
	cell::RefCell,
	rc::Rc,
};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::app::AppController;
use crate::error::Error;

pub struct DecibelConverterController {
	id: u64,
	app_controller: Rc<RefCell<AppController>>,
}

impl DecibelConverterController {
	pub fn new(id: u64, app_controller: Rc<RefCell<AppController>>) -> Rc<RefCell<Self>> {
		let controller = Rc::new(RefCell::new(DecibelConverterController {
			id,
			app_controller,
		}));
		controller
	}

	pub fn id(&self) -> u64 {
		self.id
	}

	pub fn app_controller(&self) -> &Rc<RefCell<AppController>> {
		&self.app_controller
	}

	pub fn update_min_level(&mut self, min_level: f64) -> impl Future<Output=Result<(), Error>> {
		match self.set_min_level(min_level) {
			Ok(()) => Either::Left(
				self.app_controller.borrow()
					.update_spectrum_transform(self.id)
			),
			Err(err) => Either::Right(future::err(err)),
		}
	}

	fn set_min_level(&mut self, min_level: f64) -> Result<(), Error> {
		let mut app_controller = self.app_controller.borrow_mut();
		let config = app_controller.config.spectrum_transforms.get_mut(&self.id)
			.ok_or_else(|| Error::MissingTransform { id: self.id })?;
		match config {
			SpectrumTransformConfig::DecibelConverter(config) => {
				config.min_level = min_level;
				Ok(())
			}
			config => Err(Error::UnexpectedConfigEntry(format!(
				"decibel converter control signal fired when other spectrum transform is \
				configured: {:?}",
				config
			))),
		}
	}
}