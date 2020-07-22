pub mod window;
pub mod control_pane;
pub mod visualization;

use gtk::prelude::*;
use crate::error::Error;

pub fn error_dialog(err: Error) {
	let dialog = gtk::MessageDialogBuilder::new()
		.message_type(gtk::MessageType::Error)
		.text("An unexpected system error occurred:")
		.secondary_text(&err.to_string())
		.buttons(gtk::ButtonsType::Close)
		.build();
	dialog.run();
	unsafe { dialog.destroy(); }
}
