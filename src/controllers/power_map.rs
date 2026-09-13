use futures::{future::Either, prelude::*};
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::app::AppController;
use crate::error::Error;
use crate::spectrum::TransformId;

pub struct PowerMapController {
	id: TransformId,
	app_controller: Rc<RefCell<AppController>>,
}

impl PowerMapController {
	pub fn new(id: TransformId, app_controller: Rc<RefCell<AppController>>) -> Rc<RefCell<Self>> {
		Rc::new(RefCell::new(PowerMapController { id, app_controller }))
	}

	pub fn id(&self) -> TransformId {
		self.id
	}

	pub fn app_controller(&self) -> &Rc<RefCell<AppController>> {
		&self.app_controller
	}

	pub fn update_exponent(
		&mut self,
		exponent: f64,
	) -> impl Future<Output = Result<(), Error>> + use<> {
		match self.set_exponent(exponent) {
			Ok(()) => Either::Left(
				self.app_controller
					.borrow()
					.update_spectrum_transform(self.id),
			),
			Err(err) => Either::Right(future::err(err)),
		}
	}

	fn set_exponent(&mut self, exponent: f64) -> Result<(), Error> {
		let mut app_controller = self.app_controller.borrow_mut();
		match app_controller.config.spectrum_transform_mut(self.id)? {
			SpectrumTransformConfig::PowerMap(config) => {
				config.exponent = exponent;
				Ok(())
			}
			config => Err(Error::UnexpectedConfigEntry(format!(
				"power map control signal fired when other spectrum transform is \
				configured: {:?}",
				config
			))),
		}
	}
}
