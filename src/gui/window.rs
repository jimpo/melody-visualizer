use gtk::prelude::*;
use gtk::Application;
use log::error;
use std::rc::Rc;
use std::cell::RefCell;

use crate::error::Error;
use crate::gui::control_pane;
use crate::gui::visualization;
use crate::application::Controller;

const STYLE: &[u8] = include_bytes!("style.css");
const UI_DEF: &str = include_str!("window.ui");

pub fn start<P: IsA<Application>>(app: &P) -> Result<(), Error> {
	let controller = Rc::new(RefCell::new(Controller::new()?));

	let builder = gtk::Builder::from_string(UI_DEF);
	let window: gtk::ApplicationWindow = builder.get_object("main_window").unwrap();
	let panes: gtk::Paned = builder.get_object("main_panes").unwrap();

	let (visualization_controller, visualization_widget) =
		visualization::new(controller.clone());
	panes.add1(&visualization_widget);

	let (control_pane_controller, control_pane_widget) =
		control_pane::new(controller.clone())?;
	panes.add2(&control_pane_widget);

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

	// Apply the CSS style.
	let style_provider = gtk::CssProvider::new();
	style_provider.load_from_data(STYLE).unwrap();
	gtk::StyleContext::add_provider_for_screen(
		&gdk::Screen::get_default().unwrap(),
		&style_provider,
		gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
	);

	Ok(())
}