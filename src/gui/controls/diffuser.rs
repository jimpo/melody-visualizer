use gtk::prelude::*;
use std::{
	cell::RefCell,
	rc::Rc,
};

use crate::gui::handle_async_err;
use crate::controllers::DiffuserController;

const MAX_WIDTH: f64 = 10.0;
const UI_DEF: &str = include_str!("diffuser.ui");

pub fn new(controller: &Rc<RefCell<DiffuserController>>) -> impl IsA<gtk::Widget> {
	let builder = gtk::Builder::from_string(UI_DEF);
	let view: gtk::Frame = builder.get_object("toplevel").unwrap();
	let width_scale: gtk::Scale = builder.get_object("width_scale").unwrap();

	width_scale.set_adjustment(&gtk::Adjustment::new(
		0.0,
		0.0,
		MAX_WIDTH,
		1.0,
		0.0,
		0.0
	));

	let controller_clone = controller.clone();
	width_scale.connect_change_value(
		move |_scale, _, value| on_width_change(&controller_clone, value)
	);

	view
}

fn on_width_change(controller: &Rc<RefCell<DiffuserController>>, value: f64) -> Inhibit {
	let mut controller = controller.borrow_mut();
	handle_async_err(controller.update_width(value));
	Inhibit(false)
}
