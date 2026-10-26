use futures::{future::Either, prelude::*};
use std::{cell::RefCell, rc::Rc};

use crate::app::config::SpectrumTransformConfig;
use crate::controllers::app::AppController;
use crate::error::Error;
use crate::spectrum::TransformId;
use crate::spectrum::transforms::harmonic_summation;

pub struct HarmonicSummationController {
	id: TransformId,
	app_controller: Rc<RefCell<AppController>>,
}

impl HarmonicSummationController {
	pub fn new(id: TransformId, app_controller: Rc<RefCell<AppController>>) -> Rc<RefCell<Self>> {
		Rc::new(RefCell::new(HarmonicSummationController {
			id,
			app_controller,
		}))
	}

	pub fn id(&self) -> TransformId {
		self.id
	}

	pub fn app_controller(&self) -> &Rc<RefCell<AppController>> {
		&self.app_controller
	}

	/// Applies `change` to the transform's config and ships it to the chain.
	pub fn update<Change: FnOnce(&mut harmonic_summation::Config)>(
		&mut self,
		change: Change,
	) -> impl Future<Output = Result<(), Error>> + use<Change> {
		match self.change_config(change) {
			Ok(()) => Either::Left(
				self.app_controller
					.borrow()
					.update_spectrum_transform(self.id),
			),
			Err(err) => Either::Right(future::err(err)),
		}
	}

	fn change_config(
		&mut self,
		change: impl FnOnce(&mut harmonic_summation::Config),
	) -> Result<(), Error> {
		let mut app_controller = self.app_controller.borrow_mut();
		match app_controller.config.spectrum_transform_mut(self.id)? {
			SpectrumTransformConfig::HarmonicSummation(config) => {
				change(config);
				Ok(())
			}
			config => Err(Error::UnexpectedConfigEntry(format!(
				"harmonic summation control signal fired when other spectrum transform is \
				configured: {:?}",
				config
			))),
		}
	}
}
