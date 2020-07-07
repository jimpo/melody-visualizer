mod application;
mod audio;
mod error;
mod gui;
mod graphic_renderer;
mod source;
mod spectral_renderer;

use gio::prelude::*;
use gtk::prelude::*;
use log::{info, error};
use std::rc::Rc;
use std::cell::RefCell;

use application::Controller;

use std::env;
const APP_NAME: &str = "info.jimpo.melody-visualizer";

fn main() {
    env_logger::init();

    let uiapp = gtk::Application::new(Some(APP_NAME), gio::ApplicationFlags::FLAGS_NONE)
        .expect("Application::new failed");

    let controller = Rc::new(RefCell::new(None));

    let controller_outer = controller.clone();
    uiapp.connect_activate(move |app| {
        // Create the application controller. This cannot be created outside the activate signal
        // handler as it must run within the thread owning the GTK+ default main context.
        let controller = Rc::new(RefCell::new(
            Controller::new().expect("failed to create application controller")
        ));

        // Create the GUI.
        gui::window::start(app, controller.clone())
            .expect("failed to create main window");

        // Save outer reference to controller so that it can be accessed from other Application
        // signal handlers (ie. shutdown).
        *controller_outer.borrow_mut() = Some(controller);
    });

    let controller_outer = controller.clone();
    uiapp.connect_shutdown(move |app| {
		// On shutdown we want to wait for the controller to shut down background processing
        // threads. This must be done asynchronously to avoid deadlocking.
        let main_context = glib::MainContext::default();
        let controller_outer = controller_outer.clone();
        main_context.spawn_local(async move {
            if let Some(controller) = controller_outer.borrow_mut().as_mut() {
                if let Err(err) = controller.borrow_mut().shutdown().await {
                    error!("error shutting down controller: {}", err);
                }
            }
        });
    });

    uiapp.run(&env::args().collect::<Vec<_>>());
}
