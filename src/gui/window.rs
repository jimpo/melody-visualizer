use futures::executor;
use gtk::Application;
use gtk::prelude::*;

use crate::controllers::{AppController, ControlPaneController, VisualizationController};
use crate::error::Error;
use crate::gui::control_pane;
use crate::gui::visualization;

const STYLE: &str = include_str!("style.css");
const UI_DEF: &str = include_str!("window.ui.xml");

pub fn start<P: IsA<Application>>(app: &P) -> Result<(), Error> {
	let app_controller = executor::block_on(AppController::new())?;

	let builder = gtk::Builder::from_string(UI_DEF);
	let window: gtk::ApplicationWindow = builder.object("main_window").unwrap();
	let panes: gtk::Paned = builder.object("main_panes").unwrap();

	let visualization_controller = VisualizationController::new(app_controller.clone());
	panes.set_start_child(Some(&visualization::new(&visualization_controller)));

	let control_pane_controller = ControlPaneController::new(app_controller.clone());
	panes.set_end_child(Some(&control_pane::new(&control_pane_controller)?));

	window.set_application(Some(app));
	window.present();

	window.connect_destroy(move |_| {
		app_controller.borrow_mut().shutdown();
	});

	// Apply the CSS style.
	let style_provider = gtk::CssProvider::new();
	style_provider.load_from_data(STYLE);
	gtk::style_context_add_provider_for_display(
		&gdk::Display::default().unwrap(),
		&style_provider,
		gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
	);

	Ok(())
}
