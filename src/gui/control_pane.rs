use glib::{Type, value::Value};
use gtk::{Orientation, TreeSelection, Widget};
use gtk::prelude::*;
use log::debug;
use std::cell::RefCell;
use std::rc::Rc;

use crate::error::Error;
use crate::application::{Controller, SourceType};
use jack::{PortFlags, AudioOut, PortSpec};

const STYLE: &[u8] = include_bytes!("control_pane.css");

const PORT_NAME_COL: i32 = 0;

pub struct ControlPane {
	controller: Rc<RefCell<Controller>>,
	port_store: Rc<gtk::ListStore>,
	view: gtk::Box,
}

impl ControlPane {
	pub fn new(controller: Rc<RefCell<Controller>>) -> Result<Self, Error> {
		let view = gtk::Box::new(Orientation::Vertical, 10);

		let style_provider = gtk::CssProvider::new();
		style_provider.load_from_data(STYLE)
			.map_err(Error::Glib)?;

		let style_ctx = view.get_style_context();
		style_ctx.add_provider(&style_provider, 1);
		style_ctx.add_class("control-pane");

		let source_control = gtk::Frame::new(Some("Input Source"));
		view.add(&source_control);

		let source_control_inner = gtk::Box::new(Orientation::Vertical, 10);
		source_control.add(&source_control_inner);

		let source_type_box = gtk::Box::new(Orientation::Horizontal, 0);
		source_control_inner.add(&source_type_box);

		let mut radio_button_group = None;
		for source_type in [SourceType::Audio, SourceType::MIDI].iter() {
			let selector = if let Some(ref widget) = radio_button_group {
				gtk::RadioButton::new_with_label_from_widget(widget, &source_type.to_string())
			} else {
				gtk::RadioButton::new_with_label(&source_type.to_string())
			};
			source_type_box.add(&selector);

			let is_active = controller.borrow().get_source_type() == *source_type;
			selector.set_active(is_active);

			let controller_clone = controller.clone();
			selector.connect_toggled(
				move |selector| on_source_type_toggled(&controller_clone, selector, *source_type)
			);

			radio_button_group = Some(selector)
		}

		let (port_store, port_view) = build_port_view();
		port_view.show();
		source_control_inner.add(&port_view);

		let selection = port_view.get_selection();
		selection.connect_changed(|selection| on_changed(selection));

		let port_store = Rc::new(port_store);

		{
			let controller_clone = controller.clone();
			let port_store_clone = port_store.clone();
			let callback = Box::new(move |_port_id| {
				refresh_inputs(&controller_clone.borrow(), &port_store_clone);
			});
			let mut controller = controller.borrow_mut();
			controller.subscribe_inputs_changed(callback);
		}

		refresh_inputs(&controller.borrow(), &port_store);

		Ok(ControlPane {
			controller,
			port_store,
			view,
		})
	}

	pub fn widget(&self) -> &gtk::Box {
		&self.view
	}
}

fn build_port_view() -> (gtk::ListStore, gtk::TreeView) {
	let column_types = [Type::String];

	let port_store = gtk::ListStore::new(&column_types[..]);

	let renderer = gtk::CellRendererText::new();
	let column = gtk::TreeViewColumn::new();
	column.pack_start(&renderer, true);
	column.set_title("Port");
	column.add_attribute(&renderer, "text", PORT_NAME_COL);

	let port_view = gtk::TreeView::new_with_model(&port_store);
	port_view.append_column(&column);

	(port_store, port_view)
}

fn on_changed(selection: &TreeSelection) {

}

fn on_source_type_toggled(
	controller: &RefCell<Controller>,
	selector: &gtk::RadioButton,
	source_type: SourceType
) {
	if !selector.get_active() {
		return;
	}

	let mut controller = controller.borrow_mut();
	controller.set_source_type(source_type);
}

fn refresh_inputs(controller: &Controller, port_store: &gtk::ListStore) {
	let ports = controller.jack_client()
		.ports(None, Some(AudioOut.jack_port_type()), PortFlags::IS_OUTPUT);

	// Remove rows from ListStore.
	if let Some(iter) = port_store.get_iter_first() {
		loop {
			let name = port_store
				.get_value(&iter, PORT_NAME_COL)
				.get::<String>()
				.expect("values in PORT_NAME_COL are strings")
				.expect("port names cannot be None");
			let found = ports.contains(&name);
			let iter_invalid = if !found {
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
		let found = if let Some(iter) = port_store.get_iter_first() {
			loop {
				let name = port_store
					.get_value(&iter, PORT_NAME_COL)
					.get::<String>()
					.expect("values in PORT_NAME_COL are strings")
					.expect("port names cannot be None");
				if name == new_port {
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
