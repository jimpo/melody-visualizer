pub mod control_pane;
pub mod controls;
pub mod visualization;
pub mod window;

use futures::prelude::*;
use gtk::prelude::*;

use crate::error::Error;

pub fn error_dialog(err: Error) {
	// GTK 4 removed blocking dialogs; AlertDialog shows non-modally.
	let dialog = gtk::AlertDialog::builder()
		.message("An unexpected system error occurred:")
		.detail(err.to_string())
		.modal(true)
		.build();
	dialog.show(None::<&gtk::Window>);
}

pub fn handle_async_err(fut: impl Future<Output = Result<(), Error>> + 'static) {
	glib::MainContext::default().spawn_local(async move {
		if let Err(err) = fut.await {
			error_dialog(err);
		}
	});
}
