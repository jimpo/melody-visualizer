use futures::executor;
use gtk::Application;
use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc, time::Duration};

use crate::audio::source::events::ConnectionChanged;
use crate::controllers::{AppController, ControlPaneController, VisualizationController};
use crate::error::Error;
use crate::gui::control_pane;
use crate::gui::visualization;
use crate::pubsub::SubscriptionHandle;

const STYLE: &str = include_str!("style.css");
const UI_DEF: &str = include_str!("window.ui.xml");

/// How long the floating pane toggle stays up after the pointer stops moving.
const FLOATING_TOGGLE_LINGER: Duration = Duration::from_secs(2);

pub fn start<P: IsA<Application>>(app: &P) -> Result<(), Error> {
	let app_controller = executor::block_on(AppController::new())?;

	let builder = gtk::Builder::from_string(UI_DEF);
	let window: gtk::ApplicationWindow = builder.object("main_window").unwrap();
	let header_bar: gtk::HeaderBar = builder.object("header_bar").unwrap();
	let source_label: gtk::Label = builder.object("source_label").unwrap();
	let panes: gtk::Paned = builder.object("main_panes").unwrap();
	let canvas_overlay: gtk::Overlay = builder.object("canvas_overlay").unwrap();
	let pane_toggle: gtk::ToggleButton = builder.object("pane_toggle").unwrap();
	let floating_toggle: gtk::ToggleButton = builder.object("floating_pane_toggle").unwrap();
	let floating_revealer: gtk::Revealer = builder.object("floating_toggle_revealer").unwrap();

	let visualization_controller = VisualizationController::new(app_controller.clone());
	canvas_overlay.set_child(Some(&visualization::new(&visualization_controller)));

	let control_pane_controller = ControlPaneController::new(app_controller.clone());
	let pane = control_pane::new(&control_pane_controller)?;
	panes.set_end_child(Some(&pane));

	// Hiding the pane is what collapses it to nothing: a GtkPaned gives the
	// whole width to the child that is left.
	pane_toggle
		.bind_property("active", &pane, "visible")
		.sync_create()
		.build();
	// One toggle is on screen at a time, so each has to open on the state the
	// other left.
	pane_toggle
		.bind_property("active", &floating_toggle, "active")
		.bidirectional()
		.sync_create()
		.build();

	let source_subscription = connect_source_label(&app_controller, &source_label);
	connect_fullscreen_chrome(&window, &header_bar, &canvas_overlay, &floating_revealer);

	window.set_application(Some(app));
	window.present();

	window.connect_destroy(move |_| {
		let _ = &source_subscription;
		app_controller.borrow_mut().shutdown();
	});

	// style.css carries Adwaita's dark tokens, so the widget internals it does
	// not paint itself — entry fills, scrollbars, the port list — have to come
	// from the dark theme rather than the light default.
	if let Some(settings) = gtk::Settings::default() {
		settings.set_gtk_application_prefer_dark_theme(true);
	}

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

/// Keep the header bar's subtitle on the port feeding the source.
fn connect_source_label(
	app_controller: &Rc<RefCell<AppController>>,
	label: &gtk::Label,
) -> SubscriptionHandle {
	label.set_label(&control_pane::source_name(&app_controller.borrow()));

	let app_controller_clone = app_controller.clone();
	let label = label.clone();
	app_controller
		.borrow()
		.pubsub()
		.subscribe(move |_: &ConnectionChanged| {
			label.set_label(&control_pane::source_name(&app_controller_clone.borrow()));
		})
}

/// Hand the pane toggle over to the floating button in fullscreen.
///
/// Fullscreen is the one state with no header bar to carry the toggle, so a
/// button floats over the canvas instead. It is the only chrome left on screen
/// there, and it withdraws once the pointer holds still, so it spends no longer
/// on the visualization than it has to.
fn connect_fullscreen_chrome(
	window: &gtk::ApplicationWindow,
	header_bar: &gtk::HeaderBar,
	canvas_overlay: &gtk::Overlay,
	revealer: &gtk::Revealer,
) {
	let floating = Rc::new(FloatingToggle {
		revealer: revealer.clone(),
		linger: RefCell::new(None),
	});

	let motion = gtk::EventControllerMotion::new();
	let floating_clone = floating.clone();
	motion.connect_motion(move |_controller, _x, _y| floating_clone.wake());
	canvas_overlay.add_controller(motion);

	let header_bar = header_bar.clone();
	window.connect_fullscreened_notify(move |window| {
		let fullscreen = window.is_fullscreen();
		header_bar.set_visible(!fullscreen);
		floating.revealer.set_visible(fullscreen);
		if fullscreen {
			floating.wake();
		} else {
			floating.withdraw();
		}
	});
}

/// The pane toggle that floats over the canvas in fullscreen.
struct FloatingToggle {
	revealer: gtk::Revealer,
	/// The timer that withdraws the button, while one is pending.
	linger: RefCell<Option<glib::SourceId>>,
}

impl FloatingToggle {
	/// Show the button, and start the timer that withdraws it again.
	fn wake(self: &Rc<Self>) {
		self.cancel_linger();
		self.revealer.set_reveal_child(true);

		let floating = Rc::downgrade(self);
		let linger = glib::timeout_add_local_once(FLOATING_TOGGLE_LINGER, move || {
			if let Some(floating) = floating.upgrade() {
				// The timer has fired, so there is nothing left to cancel.
				floating.linger.replace(None);
				floating.revealer.set_reveal_child(false);
			}
		});
		self.linger.replace(Some(linger));
	}

	/// Withdraw the button now, rather than waiting the timer out.
	fn withdraw(&self) {
		self.cancel_linger();
		self.revealer.set_reveal_child(false);
	}

	fn cancel_linger(&self) {
		if let Some(linger) = self.linger.replace(None) {
			linger.remove();
		}
	}
}
