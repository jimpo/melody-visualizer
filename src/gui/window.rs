use gtk::prelude::*;
use gtk::{Application, Orientation};

use crate::gui::control_pane;
use crate::gui::spiral_graphic::SpiralGraphic;

const TITLE: &str = "Melody Visualizer";

pub fn start<P: IsA<Application>>(app: &P) -> Result<(), glib::Error> {
	// We create the main window.
	let win = gtk::ApplicationWindow::new(app);

	// Then we set its size and a title.
	win.set_title(TITLE);
	win.set_default_size(1200, 800);

	let paned = gtk::Paned::new(Orientation::Horizontal);

	let graphic = SpiralGraphic::new();
	graphic.widget().show();
	paned.add1(graphic.widget());

	let control = control_pane::new()?;
	control.show();
	paned.add2(&control);

	// Set divider so left pane is square.
	paned.set_position(800);
	paned.show();

	win.add(&paned);

	// Don't forget to make all widgets visible.
	win.show_all();

	Ok(())
}