use gtk::prelude::*;
use gtk::Application;
use log::error;
use std::rc::Rc;
use std::cell::RefCell;

use crate::error::Error;
use crate::gui::control_pane::ControlPane;
use crate::gui::visualization_pane::VisualizationPane;
use crate::application::Controller;

const UI_DEF: &str = include_str!("window.ui");

pub fn start<P: IsA<Application>>(app: &P) -> Result<(), Error> {
	let controller = Rc::new(RefCell::new(Controller::new()?));

	let builder = gtk::Builder::from_string(UI_DEF);
	let window: gtk::ApplicationWindow = builder.get_object("main_window").unwrap();
	let panes: gtk::Paned = builder.get_object("main_panes").unwrap();

	let visualization = VisualizationPane::new(controller.clone());
	panes.add1(visualization.widget());

	let control = ControlPane::new(controller.clone())?;
	panes.add2(control.widget());

	window.set_application(Some(app));
	window.show_all();

	window.connect_destroy(move |_| {
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