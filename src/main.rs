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
#[cfg(test)]
mod test_support;
mod traits;

use gio::prelude::*;

const APP_NAME: &str = "info.jimpo.melody-visualizer";

fn main() {
    env_logger::init();

    let uiapp = gtk::Application::new(Some(APP_NAME), gio::ApplicationFlags::FLAGS_NONE);

    uiapp.connect_activate(move |app| {
        gui::window::start(app).expect("failed to create main window");
    });

    uiapp.run();
}