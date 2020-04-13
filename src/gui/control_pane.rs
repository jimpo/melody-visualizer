use gtk::Orientation;
use gtk::prelude::*;

const STYLE: &[u8] = include_bytes!("control_pane.css");

pub fn new() -> Result<gtk::Box, glib::Error> {
	let pane = gtk::Box::new(Orientation::Vertical, 10);

	let style_provider = gtk::CssProvider::new();
	style_provider.load_from_data(STYLE)?;

	let style_ctx = pane.get_style_context();
	style_ctx.add_provider(&style_provider, 1);
	style_ctx.add_class("control-pane");

	Ok(pane)
}