use gio::prelude::*;
use melody_visualizer::gui;

const APP_NAME: &str = "info.jimpo.melody-visualizer";

fn main() {
	env_logger::init();

	let uiapp = gtk::Application::new(Some(APP_NAME), gio::ApplicationFlags::FLAGS_NONE);

	uiapp.connect_activate(move |app| {
		gui::window::start(app).expect("failed to create main window");
	});

	uiapp.run();
}
