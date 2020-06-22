use std::cell::RefCell;
use std::rc::Rc;use glib::Type;
use log::info;
use gtk::{Orientation, TreeSelection, Widget};
use gtk::prelude::*;

use crate::error::Error;
use crate::application::{Controller, SourceType};

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

		let source_control = gtk::Frame::new(Some("Input Source"));
		view.add(&source_control);

		let source_control_inner = gtk::Box::new(Orientation::Vertical, 10);
		source_control.add(&source_control_inner);

		let source_type_box = gtk::Box::new(Orientation::Horizontal, 0);
		source_control_inner.add(&source_type_box);

		let audio_selector = gtk::RadioButton::new_with_label("Audio");
		let midi_selector = gtk::RadioButton::new_with_label_from_widget(&audio_selector, "MIDI");
		source_type_box.add(&audio_selector);
		source_type_box.add(&midi_selector);

		audio_selector.connect_toggled(
			|selector| on_source_type_toggled(selector, SourceType::Audio)
		);
		midi_selector.connect_property_active_notify(
			|selector| on_source_type_toggled(selector, SourceType::MIDI)
		);

		let (port_store, port_view) = build_port_view();
		port_view.show();
		source_control_inner.add(&port_view);

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

fn build_port_view() -> (gtk::ListStore, gtk::TreeView) {
	let column_types = [Type::String];

	let port_store = gtk::ListStore::new(&column_types[..]);

	let renderer = gtk::CellRendererText::new();
	let column = gtk::TreeViewColumn::new();
	column.pack_start(&renderer, true);
	column.set_title("Port");
	column.add_attribute(&renderer, "text", 0);

	let port_view = gtk::TreeView::new_with_model(&port_store);
	port_view.append_column(&column);

	(port_store, port_view)
}

fn on_changed(selection: &TreeSelection) {

}

fn on_source_type_toggled(selector: &gtk::RadioButton, source_type: SourceType) {
	if !selector.get_active() {
		return;
	}
	debug!("Source type \"{:?}\" activated", source_type);
}