use gio::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::audio::source::PortName;
use crate::audio::source::events::PortsChanged;
use crate::controllers::app::AppController;
use crate::pubsub::SubscriptionHandle;

pub struct ControlPaneController {
	app_controller: Rc<RefCell<AppController>>,
	/// The ports the source can be fed from, as a list model for the port
	/// list to bind to.
	ports: gtk::StringList,
	ports_changed_subscription: Option<SubscriptionHandle>,
}

impl ControlPaneController {
	pub fn new(app_controller: Rc<RefCell<AppController>>) -> Rc<RefCell<Self>> {
		let controller = Rc::new(RefCell::new(ControlPaneController {
			app_controller: app_controller.clone(),
			ports: gtk::StringList::new(&[]),
			ports_changed_subscription: None,
		}));

		// Refresh port list when JACK's set of ports changes.
		let controller_clone = controller.clone();
		let subscription = app_controller
			.borrow()
			.pubsub()
			.subscribe(move |_: &PortsChanged| controller_clone.borrow().refresh_inputs());

		{
			let mut controller = controller.borrow_mut();
			controller.ports_changed_subscription = Some(subscription);

			// Populate the initial port list.
			controller.refresh_inputs();
		}

		controller
	}

	pub fn ports(&self) -> &gtk::StringList {
		&self.ports
	}

	/// The port at `position` in the list, as the port list numbers its rows.
	pub fn port_at(&self, position: u32) -> Option<PortName> {
		self.ports
			.string(position)
			.map(|name| PortName::from(name.to_string()))
	}

	pub fn app_controller(&self) -> &Rc<RefCell<AppController>> {
		&self.app_controller
	}

	/// Bring the list in line with the ports JACK offers.
	///
	/// Rows are added and removed rather than rebuilt so that the row the user
	/// selected keeps its selection.
	fn refresh_inputs(&self) {
		let available = self.app_controller.borrow().available_inputs();
		let listed = (0..self.ports.n_items())
			.filter_map(|position| self.port_at(position))
			.collect::<Vec<_>>();

		// Walk backwards, so a removal leaves every earlier position valid.
		for (position, port) in listed.iter().enumerate().rev() {
			if !available.contains(port) {
				log::debug!("removing port {port} from the list");
				self.ports.remove(position as u32);
			}
		}

		for port in &available {
			if !listed.contains(port) {
				self.ports.append(port.as_str());
			}
		}
	}
}
