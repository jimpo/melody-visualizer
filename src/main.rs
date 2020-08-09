mod app;
mod controllers;
mod async_processor;
mod audio;
mod error;
mod gui;
mod graphic;
mod note;
mod pubsub;
mod source;
mod spectrum;
mod traits;

use gio::prelude::*;

use std::env;
const APP_NAME: &str = "info.jimpo.melody-visualizer";

fn main() {
    env_logger::init();

    let uiapp = gtk::Application::new(Some(APP_NAME), gio::ApplicationFlags::FLAGS_NONE)
        .expect("Application::new failed");

    uiapp.connect_activate(move |app| {
        gui::window::start(app).expect("failed to create main window");
    });

    uiapp.run(&env::args().collect::<Vec<_>>());
}