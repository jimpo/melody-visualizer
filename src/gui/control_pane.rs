use glib::Type;
use gtk::{Orientation, TreeSelection};
use gtk::prelude::*;

const STYLE: &[u8] = include_bytes!("control_pane.css");

pub fn new() -> Result<gtk::Box, glib::Error> {
	let pane = gtk::Box::new(Orientation::Vertical, 10);

	let style_provider = gtk::CssProvider::new();
	style_provider.load_from_data(STYLE)?;

	let style_ctx = pane.get_style_context();
	style_ctx.add_provider(&style_provider, 1);
	style_ctx.add_class("control-pane");

	let column_types = [Type::String];

	let list_store = gtk::ListStore::new(&column_types[..]);

	let renderer = gtk::CellRendererText::new();
	let column = gtk::TreeViewColumn::new();
	column.pack_start(&renderer, true);
	column.set_title("Port");
	column.add_attribute(&renderer, "text", 0);

	let list_view = gtk::TreeView::new_with_model(&list_store);
	list_view.append_column(&column);
	list_view.show();

	pane.add(&list_view);

	let selection = list_view.get_selection();
	selection.connect_changed(on_changed);

	Ok(pane)
}

fn on_changed(selection: &TreeSelection) {

}