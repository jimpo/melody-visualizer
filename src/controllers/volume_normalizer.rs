use futures::{future::Either, prelude::*};
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::app::AppController;
use crate::error::Error;

pub struct VolumeNormalizerController {
	id: u64,
	app_controller: Rc<RefCell<AppController>>,
}

impl VolumeNormalizerController {
	pub fn new(id: u64, app_controller: Rc<RefCell<AppController>>) -> Rc<RefCell<Self>> {
		let controller = Rc::new(RefCell::new(VolumeNormalizerController {
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

	pub fn update_rate(&mut self, rate: f64) -> impl Future<Output = Result<(), Error>> + use<> {
		match self.set_rate(rate) {
			Ok(()) => Either::Left(
				self.app_controller
					.borrow()
					.update_spectrum_transform(self.id),
			),
			Err(err) => Either::Right(future::err(err)),
		}
	}

	fn set_rate(&mut self, rate: f64) -> Result<(), Error> {
		let mut app_controller = self.app_controller.borrow_mut();
		let config = app_controller
			.config
			.spectrum_transforms
			.get_mut(&self.id)
			.ok_or_else(|| Error::MissingTransform { id: self.id })?;
		match config {
			SpectrumTransformConfig::VolumeNormalizer(config) => {
				config.rate = rate;
				Ok(())
			}
			config => Err(Error::UnexpectedConfigEntry(format!(
				"volume normalizer control signal fired when other spectrum transform is \
				configured: {:?}",
				config
			))),
		}
	}
}
