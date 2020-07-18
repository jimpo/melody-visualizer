mod application;
mod async_processor;
mod audio;
mod audio_spectrum_generator;
mod error;
mod gui;
mod graphic;
mod graphic_renderer;
mod note;
mod pubsub;
mod source;
mod spectrum;
mod spectrum_renderer;
mod spiral;

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
