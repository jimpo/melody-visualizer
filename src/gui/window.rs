use gtk::prelude::*;
use gtk::Application;
use futures::executor;

use crate::error::Error;
use crate::gui::control_pane;
use crate::gui::visualization;
use crate::controllers::{AppController, ControlPaneController, VisualizationController};

const STYLE: &[u8] = include_bytes!("style.css");
const UI_DEF: &str = include_str!("window.ui");

pub fn start<P: IsA<Application>>(app: &P) -> Result<(), Error> {
	let app_controller = executor::block_on(AppController::new())?;

	let builder = gtk::Builder::from_string(UI_DEF);
	let window: gtk::ApplicationWindow = builder.get_object("main_window").unwrap();
	let panes: gtk::Paned = builder.get_object("main_panes").unwrap();

	let visualization_controller = VisualizationController::new(app_controller.clone());
	panes.add1(&visualization::new(&visualization_controller));

	let control_pane_controller = ControlPaneController::new(app_controller.clone());
	panes.add2(&control_pane::new(&control_pane_controller)?);

	window.set_application(Some(app));
	window.show_all();

	window.connect_destroy(move |_| {
		// On shutdown we want to wait for the controller to shut down background processing
		// threads. This must be done asynchronously to avoid deadlocking.
		let main_context = glib::MainContext::default();
		let controller_clone = app_controller.clone();
		main_context.spawn_local(async move {
			if let Err(err) = controller_clone.borrow_mut().shutdown().await {
				log::error!("error shutting down controller: {}", err);
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