use glib::Type;
use gtk::{prelude::*, TreeIter};
use jack::{AudioOut, PortFlags, PortSpec};
use std::{
	cell::RefCell,
	rc::Rc,
};

use crate::controllers::app::AppController;
use crate::pubsub::SubscriptionHandle;
use crate::source::{events::InputsChanged};

pub const PORT_NAME_COL: i32 = 0;

pub struct ControlPaneController {
	app_controller: Rc<RefCell<AppController>>,
	port_store: gtk::ListStore,
	inputs_changed_subscription: Option<SubscriptionHandle>,
}

impl ControlPaneController {
	pub fn new(app_controller: Rc<RefCell<AppController>>) -> Rc<RefCell<Self>> {
		let column_types = [Type::STRING];
		let port_store = gtk::ListStore::new(&column_types[..]);

		let controller = Rc::new(RefCell::new(ControlPaneController {
			app_controller: app_controller.clone(),
			port_store,
			inputs_changed_subscription: None,
		}));

		// Refresh port list when JACK inputs change.
		let controller_clone = controller.clone();
		let subscription = app_controller.borrow()
			.pubsub()
			.subscribe(move |notification: &InputsChanged| {
				on_input_ports_changed(&controller_clone, notification.clone());
				controller_clone.borrow().refresh_inputs()
			});

		{
			let mut controller = controller.borrow_mut();
			controller.inputs_changed_subscription = Some(subscription);

			// Populate the initial port list.
			controller.refresh_inputs();
		}

		controller
	}

	pub fn port_store(&self) -> &gtk::ListStore {
		&self.port_store
	}

	pub fn app_controller(&self) -> &Rc<RefCell<AppController>> {
		&self.app_controller
	}

	fn refresh_inputs(&self) {
		let port_store = &self.port_store;

		let ports = self.app_controller.borrow()
			.jack_client()
			.map(|client| client.ports(None, Some(AudioOut::default().jack_port_type()), PortFlags::IS_OUTPUT))
			.unwrap_or_default();

		// Remove rows from ListStore.
		if let Some(iter) = port_store.iter_first() {
			loop {
				let found = ports.contains(&get_port_name(port_store, &iter));
				let iter_invalid = if !found {
					log::debug!("attempting to remove port");
					port_store.remove(&iter)
				} else {
					port_store.iter_next(&iter)
				};
				if !iter_invalid {
					break;
				}
			}
		}

		// Add rows to ListStore.
		for new_port in ports {
			let found = if let Some(iter) = port_store.iter_first() {
				loop {
					if new_port == get_port_name(port_store, &iter) {
						break true;
					} else if !port_store.iter_next(&iter) {
						break false;
					}
				}
			} else {
				false
			};
			if !found {
				let iter = port_store.append();
				port_store.set_value(&iter, PORT_NAME_COL as u32, &new_port.to_value());
			}
		}
	}
}

fn on_input_ports_changed(
	controller_ref: &Rc<RefCell<ControlPaneController>>,
	update: InputsChanged,
) {
	// TODO: Unfortunately, we need to poll until port_update is reflected.
	// https://github.com/jackaudio/jack2/issues/617
	let controller = controller_ref.clone();
	glib::timeout_add_local(std::time::Duration::from_millis(10), move || {
		if is_inputs_update_pending(&*controller.borrow(), update.clone()) {
			glib::ControlFlow::Continue
		} else {
			controller.borrow().refresh_inputs();
			glib::ControlFlow::Break
		}
	});
}

fn is_inputs_update_pending(controller: &ControlPaneController, update: InputsChanged) -> bool {
	controller
		.app_controller.borrow()
		.jack_client()
		.map(move |client| {
			match update {
				// https://github.com/jackaudio/jack2/issues/617
				InputsChanged::Unregistered(port_id) => {
					if let Some(port) = client.port_by_id(port_id) {
						match port.name() {
							Ok(name) =>
								client
									.ports(None, None, PortFlags::empty())
									.contains(&name),
							Err(err) => {
								log::warn!("JACK port {} has no name: {}", port_id, err);
								// Whatever, let's just say it's updated.
								false
							}
						}
					} else {
						false
					}
				}
				// I don't think we need to double-check any other cases.
				_ => false,
			}
		})
		.unwrap_or(false)
}

fn get_port_name<TM: TreeModelExt>(port_store: &TM, iter: &TreeIter) -> String {
	port_store
		.value(&iter, PORT_NAME_COL)
		.get::<String>()
		.expect("values in PORT_NAME_COL are strings")
}
