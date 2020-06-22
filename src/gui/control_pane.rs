use std::cell::RefCell;
use std::rc::Rc;use glib::Type;
use gtk::{Orientation, TreeSelection};
use gtk::prelude::*;

use crate::error::Error;
use crate::application::Controller;

const STYLE: &[u8] = include_bytes!("control_pane.css");

pub struct ControlPane {
	controller: Rc<RefCell<Controller>>,
	port_store: gtk::ListStore,
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

		let column_types = [Type::String];

		let port_store = gtk::ListStore::new(&column_types[..]);

		let renderer = gtk::CellRendererText::new();
		let column = gtk::TreeViewColumn::new();
		column.pack_start(&renderer, true);
		column.set_title("Port");
		column.add_attribute(&renderer, "text", 0);

		let port_view = gtk::TreeView::new_with_model(&port_store);
		port_view.append_column(&column);
		port_view.show();

		view.add(&port_view);

		let selection = port_view.get_selection();
		selection.connect_changed(|selection| on_changed(selection));

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

fn on_changed(selection: &TreeSelection) {

}