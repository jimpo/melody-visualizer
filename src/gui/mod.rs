pub mod window;
pub mod control_pane;
pub mod controls;
pub mod visualization;

use gtk::prelude::*;
use futures::prelude::*;

use crate::error::Error;

pub fn error_dialog(err: Error) {
	let dialog = gtk::MessageDialog::builder()
		.message_type(gtk::MessageType::Error)
		.text("An unexpected system error occurred:")
		.secondary_text(&err.to_string())
		.buttons(gtk::ButtonsType::Close)
		.build();
	dialog.run();
	dialog.close();
}

pub fn handle_async_err(fut: impl Future<Output=Result<(), Error>> + 'static) {
	glib::MainContext::default().spawn_local(async move {
		if let Err(err) = fut.await {
			error_dialog(err);
		}
	});
}