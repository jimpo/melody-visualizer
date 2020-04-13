use std::cell::RefCell;
use std::rc::Rc;
use gtk::prelude::*;

struct WidgetState {
	frames_since_last_buffer: usize,
}

pub struct SpiralGraphic {
	state: Rc<RefCell<WidgetState>>,
	drawing_area: gtk::DrawingArea,
}

impl SpiralGraphic {
	pub fn new() -> Self {
		let state = WidgetState {
			frames_since_last_buffer: 0,
		};
		let graphic = SpiralGraphic {
			state: Rc::new(RefCell::new(state)),
			drawing_area: gtk::DrawingArea::new(),
		};

		let area = &graphic.drawing_area;

		let state = graphic.state.clone();
		area.connect_size_allocate(
			move |area, alloc| on_size_allocate(&mut state.borrow_mut(), area, alloc)
		);

		let state = graphic.state.clone();
		area.connect_draw(
			move |area, ctx| on_draw(&mut state.borrow_mut(), area, ctx)
		);

		graphic
	}

	pub fn widget(&self) -> &gtk::DrawingArea {
		&self.drawing_area
	}
}

fn on_size_allocate(
	state: &mut WidgetState,
	area: &gtk::DrawingArea,
	allocation: &gtk::Allocation
) {

}

fn on_draw(state: &mut WidgetState, area: &gtk::DrawingArea, ctx: &cairo::Context) -> Inhibit {
	let x_max = area.get_allocated_width();
	let y_max = area.get_allocated_height();

	ctx.set_source_rgb(0.0, 0.0, 0.0);
	ctx.rectangle(0.0, 0.0, x_max as f64, y_max as f64);
	ctx.fill();

	Inhibit(false)
}

