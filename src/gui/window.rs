use gtk::prelude::*;
use gtk::{Application, Orientation};
use log::error;
use std::rc::Rc;
use std::cell::RefCell;

use crate::error::Error;
use crate::gui::control_pane::ControlPane;
use crate::gui::visualization_pane::VisualizationPane;
use crate::application::Controller;

const TITLE: &str = "Melody Visualizer";

pub fn start<P: IsA<Application>>(app: &P) -> Result<(), Error> {
	let controller = Rc::new(RefCell::new(Controller::new()?));

	// Create the main window.
	let win = gtk::ApplicationWindow::new(app);

	// Then we set its size and a title.
	win.set_title(TITLE);
	win.set_default_size(1200, 800);

	let paned = gtk::Paned::new(Orientation::Horizontal);

	let visualization = VisualizationPane::new(controller.clone());
	visualization.widget().show();
	paned.add1(visualization.widget());

	let control = ControlPane::new(controller.clone())?;
	control.widget().show();
	paned.add2(control.widget());

	// Set divider so left pane is square.
	paned.set_position(800);
	paned.show();

	win.add(&paned);

	// Don't forget to make all widgets visible.
	win.show_all();

	win.connect_destroy(move |_| {
		// On shutdown we want to wait for the controller to shut down background processing
		// threads. This must be done asynchronously to avoid deadlocking.
		let main_context = glib::MainContext::default();
		let controller_clone = controller.clone();
		main_context.spawn_local(async move {
			if let Err(err) = controller_clone.borrow_mut().shutdown().await {
				error!("error shutting down controller: {}", err);
			}
		});
	});

	Ok(())
}