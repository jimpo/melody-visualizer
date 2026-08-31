use futures::executor;
use gtk::Application;
use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

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
		// On shutdown we want to wait for the controller to shut down background processing
		// threads. This must be done asynchronously to avoid deadlocking.
		let main_context = glib::MainContext::default();
		main_context.spawn_local(shutdown(app_controller.clone()));
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

/// Stops the controller's background processing threads.
///
/// The controller stays borrowed across the await. Taking the stop futures out
/// of the borrow would mean moving the renderers out of the controller:
/// `AsyncProcessor::stop` disconnects one sender, so stopping a clone would
/// leave the controller's own sender open and the renderer threads running.
/// Nothing else borrows the controller here, because the window that owns every
/// other borrow is already destroyed.
#[allow(clippy::await_holding_refcell_ref)]
async fn shutdown(controller: Rc<RefCell<AppController>>) {
	if let Err(err) = controller.borrow_mut().shutdown().await {
		log::error!("error shutting down controller: {}", err);
	}
}
