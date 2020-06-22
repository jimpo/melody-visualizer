mod application;
mod audio;
mod error;
mod gui;

use gio::prelude::*;
use gtk::prelude::*;
use log::info;

use std::env;
const APP_NAME: &str = "info.jimpo.melody-visualizer";

fn main() {
    env_logger::init();

    let uiapp = gtk::Application::new(Some(APP_NAME), gio::ApplicationFlags::FLAGS_NONE)
        .expect("Application::new failed");

    uiapp.connect_activate(
        |app| gui::window::start(app).expect("Failed to create main window")
    );
    uiapp.run(&env::args().collect::<Vec<_>>());
}
