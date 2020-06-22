use gtk::prelude::*;
use gtk::{Application, Orientation};
use std::rc::Rc;
use std::cell::RefCell;

use crate::error::Error;
use crate::gui::control_pane::ControlPane;
use crate::gui::visualization_pane::VisualizationPane;
use crate::application::Controller;

const TITLE: &str = "Melody Visualizer";

pub fn start<P: IsA<Application>>(app: &P) -> Result<(), Error> {
	// We create the main window.
	let win = gtk::ApplicationWindow::new(app);

	let controller = Rc::new(RefCell::new(Controller::new()));

	// Then we set its size and a title.
	win.set_title(TITLE);
	win.set_default_size(1200, 800);

	let paned = gtk::Paned::new(Orientation::Horizontal);

	let graphic = VisualizationPane::new(controller.clone());
	graphic.widget().show();
	paned.add1(graphic.widget());

	let control = ControlPane::new(controller.clone())?;
	control.widget().show();
	paned.add2(control.widget());

	// Set divider so left pane is square.
	paned.set_position(800);
	paned.show();

	win.add(&paned);

	// Don't forget to make all widgets visible.
	win.show_all();

	Ok(())
}