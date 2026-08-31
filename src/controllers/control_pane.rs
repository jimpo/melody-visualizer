// The port list uses GtkTreeView/GtkListStore, deprecated in GTK 4 (see the note
// in src/gui/control_pane.rs). Migrating to GtkColumnView is future work.
#![allow(deprecated)]

use glib::Type;
use gtk::{TreeIter, prelude::*};
use std::{cell::RefCell, rc::Rc};

use crate::audio::source::PortName;
use crate::audio::source::events::PortsChanged;
use crate::controllers::app::AppController;
use crate::pubsub::SubscriptionHandle;

pub const PORT_NAME_COL: i32 = 0;

pub struct ControlPaneController {
	app_controller: Rc<RefCell<AppController>>,
	port_store: gtk::ListStore,
	ports_changed_subscription: Option<SubscriptionHandle>,
}

impl ControlPaneController {
	pub fn new(app_controller: Rc<RefCell<AppController>>) -> Rc<RefCell<Self>> {
		let column_types = [Type::STRING];
		let port_store = gtk::ListStore::new(&column_types[..]);

		let controller = Rc::new(RefCell::new(ControlPaneController {
			app_controller: app_controller.clone(),
			port_store,
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

	pub fn port_store(&self) -> &gtk::ListStore {
		&self.port_store
	}

	pub fn app_controller(&self) -> &Rc<RefCell<AppController>> {
		&self.app_controller
	}

	/// Bring the list store in line with the ports JACK offers.
	///
	/// Rows are added and removed rather than rebuilt so that the row the user
	/// selected keeps its selection.
	fn refresh_inputs(&self) {
		let port_store = &self.port_store;
		let ports = self.app_controller.borrow().available_inputs();

		// Remove rows from ListStore.
		if let Some(mut iter) = port_store.iter_first() {
			loop {
				let found = ports.contains(&get_port_name(port_store, &iter));
				let iter_invalid = if !found {
					log::debug!("attempting to remove port");
					port_store.remove(&iter)
				} else {
					port_store.iter_next(&mut iter)
				};
				if !iter_invalid {
					break;
				}
			}
		}

		// Add rows to ListStore.
		for new_port in ports {
			let found = if let Some(mut iter) = port_store.iter_first() {
				loop {
					if new_port == get_port_name(port_store, &iter) {
						break true;
					} else if !port_store.iter_next(&mut iter) {
						break false;
					}
				}
			} else {
				false
			};
			if !found {
				let iter = port_store.append();
				port_store.set_value(&iter, PORT_NAME_COL as u32, &new_port.as_str().to_value());
			}
		}
	}
}

fn get_port_name<TM: IsA<gtk::TreeModel>>(port_store: &TM, iter: &TreeIter) -> PortName {
	port_store
		.get_value(iter, PORT_NAME_COL)
		.get::<String>()
		.expect("values in PORT_NAME_COL are strings")
		.into()
}
