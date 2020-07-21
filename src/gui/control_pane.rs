use futures::prelude::*;
use glib::Type;
use gtk::{prelude::*, TreeSelection, TreeIter};
use std::cell::RefCell;
use std::rc::Rc;

use crate::note; // TODO: Rename this macro to not conflict with module.
use crate::application::Controller;
use crate::async_processor::AsyncProcessor;
use crate::error::Error;
use crate::graphic_renderer::{GraphicRendererCmd, GraphicRenderer};
use crate::note::Note;
use crate::spectrum::SpectrumParams;
use crate::source::{events::InputsChanged, SourceType};
use crate::spiral::{self, SpiralGenerator};
use jack::{PortFlags, AudioOut, PortSpec};

const STYLE: &[u8] = include_bytes!("control_pane.css");
const UI_DEF: &str = include_str!("control_pane.ui");

const PORT_NAME_COL: i32 = 0;

const MIN_NOTE: Note = note!(A, 0);
const MAX_NOTE: Note = note!(C, 8);

const DEFAULT_MIN_FREQ: f64 = 200.0; // Hz
const DEFAULT_MAX_FREQ: f64 = 2000.0; // Hz
const DEFAULT_SAMPLES_PER_OCTAVE: usize = 180;
const DEFAULT_SPIRAL_KEY_FREQ: f64 = 263.74; // C
const DEFAULT_SPIRAL_OUTER_PAD: f64 = 20.0;
const DEFAULT_SPIRAL_INNER_PAD: f64 = 50.0;

pub struct ControlPane {
	app_controller: Rc<RefCell<Controller>>,
	local_controller: Rc<RefCell<ControlPaneController>>,
	view: gtk::Box,
}

impl ControlPane {
	pub fn new(app_controller: Rc<RefCell<Controller>>) -> Result<Self, Error> {
		let local_controller = ControlPaneController::new(app_controller.clone());

		let builder = gtk::Builder::from_string(UI_DEF);
		let view: gtk::Box = builder.get_object("control_pane").unwrap();
		let source_type_selection: gtk::Box = builder.get_object("source_type_selection").unwrap();
		let port_view: gtk::TreeView = builder.get_object("port_list").unwrap();
		let min_freq_scale: gtk::Scale = builder.get_object("min_freq_scale").unwrap();
		let max_freq_scale: gtk::Scale = builder.get_object("max_freq_scale").unwrap();
		let key_freq_scale: gtk::Scale = builder.get_object("key_freq_scale").unwrap();

		// Style the control pane.
		let style_provider = gtk::CssProvider::new();
		style_provider.load_from_data(STYLE)
			.map_err(Error::Glib)?;

		// Populate source selection radio buttons.
		for selector in build_source_type_selectors(&app_controller) {
			source_type_selection.add(&selector);
		}

		port_view.set_model(Some(local_controller.borrow().port_store()));

		min_freq_scale.set_adjustment(&gtk::Adjustment::new(
			MIN_NOTE.log_frequency(),
			MIN_NOTE.log_frequency(),
			MAX_NOTE.log_frequency(),
			1.0 / 12.0,
			0.0,
			0.0
		));
		max_freq_scale.set_adjustment(&gtk::Adjustment::new(
			MIN_NOTE.log_frequency(),
			MIN_NOTE.log_frequency(),
			MAX_NOTE.log_frequency(),
			1.0 / 12.0,
			0.0,
			0.0
		));
		key_freq_scale.set_adjustment(&gtk::Adjustment::new(
			note!(C, 3).log_frequency(),
			note!(C, 3).log_frequency(),
			note!(C, 4).log_frequency(),
			1.0 / 12.0,
			0.0,
			0.0
		));

		let controller_clone = local_controller.clone();
		min_freq_scale.connect_change_value(
			move |_scale, _, value| on_min_freq_change(&controller_clone, value)
		);

		let controller_clone = local_controller.clone();
		max_freq_scale.connect_change_value(
			move |_scale, _, value| on_max_freq_change(&controller_clone, value)
		);

		let controller_clone = local_controller.clone();
		key_freq_scale.connect_change_value(
			move |_scale, _, value| on_key_freq_change(&controller_clone, value)
		);

		{
			let controller = local_controller.borrow();
			min_freq_scale.set_value(controller.min_log_freq);
			max_freq_scale.set_value(controller.max_log_freq);
			key_freq_scale.set_value(controller.key_log_freq);
		}

		let selection = port_view.get_selection();
		let controller_clone = app_controller.clone();
		selection.connect_changed(move |selection| on_port_selected(&controller_clone, selection));

		// Refresh port list when JACK inputs change.
		let app_controller_clone = app_controller.clone();
		let local_controller_clone = local_controller.clone();
		app_controller.borrow()
			.pubsub()
			.subscribe(move |_: &InputsChanged| {
				local_controller_clone.borrow()
					.refresh_inputs(&app_controller_clone.borrow());
			});

		// Populate the initial port list.
		local_controller.borrow()
			.refresh_inputs(&app_controller.borrow());

		Ok(ControlPane {
			app_controller,
			local_controller,
			view,
		})
	}

	// TODO: Make this Deref<Target = gtk::Box>
	pub fn widget(&self) -> &gtk::Box {
		&self.view
	}
}

fn build_port_view(port_store: &gtk::ListStore) -> gtk::TreeView {
	let renderer = gtk::CellRendererText::new();
	let column = gtk::TreeViewColumn::new();
	column.pack_start(&renderer, true);
	column.set_title("Port");
	column.add_attribute(&renderer, "text", PORT_NAME_COL);

	let port_view = gtk::TreeView::with_model(port_store);
	port_view.append_column(&column);
	port_view
}

fn on_port_selected(controller: &RefCell<Controller>, selection: &TreeSelection) {
	let port_name = selection.get_selected()
		.map(|(port_store, iter)| get_port_name(&port_store, &iter));
	let mut controller = controller.borrow_mut();
	controller.connect_port(port_name);
}

fn on_source_type_toggled(
	controller: &RefCell<Controller>,
	selector: &gtk::RadioButton,
	source_type: SourceType
) {
	if !selector.get_active() {
		return;
	}

	let mut controller = controller.borrow_mut();
	if let Err(err) = controller.set_source_type(source_type) {
		log::error!("failed to change source type: {}", err);
	}
}

fn on_min_freq_change(controller_ref: &Rc<RefCell<ControlPaneController>>, value: f64) -> Inhibit {
	let mut controller = controller_ref.borrow_mut();
	if value > controller.max_log_freq {
		return Inhibit(true);
	}

	controller.min_log_freq = value;
	let async_update = controller.update_spectrum_params();

	let main_context = glib::MainContext::default();
	main_context.spawn_local(async move {
		// TODO: Handle errors better
		async_update.await.unwrap();
	});

	Inhibit(false)
}

fn on_max_freq_change(controller_ref: &Rc<RefCell<ControlPaneController>>, value: f64) -> Inhibit {
	let mut controller = controller_ref.borrow_mut();
	if value < controller.min_log_freq {
		return Inhibit(true);
	}

	controller.max_log_freq = value;
	let async_update = controller.update_spectrum_params();

	let main_context = glib::MainContext::default();
	main_context.spawn_local(async move {
		// TODO: Handle errors better
		async_update.await.unwrap();
	});

	Inhibit(false)
}


fn on_key_freq_change(controller_ref: &Rc<RefCell<ControlPaneController>>, value: f64) -> Inhibit {
	let mut controller = controller_ref.borrow_mut();

	controller.key_log_freq = value;
	let async_update = controller.update_spiral_config();

	let main_context = glib::MainContext::default();
	main_context.spawn_local(async move {
		// TODO: Handle errors better
		async_update.await.unwrap();
	});

	Inhibit(false)
}

struct ControlPaneController {
	port_store: gtk::ListStore,
	min_log_freq: f64,
	max_log_freq: f64,
	key_log_freq: f64,
	samples_per_octave: usize,
	graphic_renderer: AsyncProcessor<GraphicRendererCmd, GraphicRenderer>,
}

impl ControlPaneController {
	fn new(app_controller: Rc<RefCell<Controller>>) -> Rc<RefCell<Self>> {
		let column_types = [Type::String];
		let port_store = gtk::ListStore::new(&column_types[..]);

		let app_controller = app_controller.borrow();
		let graphic_renderer = app_controller.graphic_renderer().clone();

		let controller_ref = Rc::new(RefCell::new(ControlPaneController {
			port_store,
			min_log_freq: DEFAULT_MIN_FREQ.log2(),
			max_log_freq: DEFAULT_MAX_FREQ.log2(),
			key_log_freq: DEFAULT_SPIRAL_KEY_FREQ.log2(),
			samples_per_octave: DEFAULT_SAMPLES_PER_OCTAVE,
			graphic_renderer,
		}));

		{
			let mut controller = controller_ref.borrow_mut();
			let async_spectrum_params_update = controller.update_spectrum_params();
			let async_generator_update = controller.update_graphic_generator();
			let async_spiral_config_update = controller.update_spectrum_params();

			let main_context = glib::MainContext::default();
			main_context.spawn_local(async move {
				// TODO: Handle errors better
				async_spectrum_params_update.await.unwrap();
				async_generator_update.await.unwrap();
				async_spiral_config_update.await.unwrap();
			});
		}

		controller_ref
	}

	// TODO: Move this to an architecture overview or something.
	//
	// We have to be very careful about RefCells in async code. So borrowing a controller from a
	// RefCell then yielding with await is a big problem.
	fn update_spectrum_params(&self) -> impl Future<Output=Result<(), Error>> {
		let spectrum_params = self.build_spectrum_params();
		self.graphic_renderer.call_cloned::<()>(
			GraphicRendererCmd::SetSpectrumParams(spectrum_params)
		)
	}

	fn update_spiral_config(&self) -> impl Future<Output=Result<(), Error>> {
		let config = self.build_spiral_config();
		self.graphic_renderer.call_cloned::<Result<(), Error>>(
			GraphicRendererCmd::CallGenerator(Box::new(spiral::SpiralCmd::SetConfig(config)))
		)
			.map(|result| result.unwrap())
	}

	fn update_graphic_generator(&self) -> impl Future<Output=Result<(), Error>> {
		let spiral = SpiralGenerator::new(self.build_spiral_config());
		self.graphic_renderer.call_cloned::<()>(
			GraphicRendererCmd::SetGenerator(Box::new(spiral))
		)
	}

	fn port_store(&self) -> &gtk::ListStore {
		&self.port_store
	}

	fn refresh_inputs(&self, controller: &Controller) {
		let port_store = &self.port_store;

		let ports = controller.jack_client()
			.map(|client| client.ports(None, Some(AudioOut.jack_port_type()), PortFlags::IS_OUTPUT))
			.unwrap_or_default();

		// Remove rows from ListStore.
		if let Some(iter) = port_store.get_iter_first() {
			loop {
				let found = ports.contains(&get_port_name(port_store, &iter));
				let iter_invalid = if !found {
					port_store.remove(&iter)
				} else {
					port_store.iter_next(&iter)
				};
				if !iter_invalid {
					break;
				}
			}
		}

		// Add rows to ListStore.
		for new_port in ports {
			let found = if let Some(iter) = port_store.get_iter_first() {
				loop {
					if new_port == get_port_name(port_store, &iter) {
						break true;
					} else if !port_store.iter_next(&iter) {
						break false;
					}
				}
			} else {
				false
			};
			if !found {
				let iter = port_store.append();
				port_store.set_value(&iter, PORT_NAME_COL as u32, &new_port.to_value());
			}
		}
	}

	fn build_spectrum_params(&self) -> SpectrumParams {
		let octaves = self.max_log_freq - self.min_log_freq;
		let samples = (self.samples_per_octave as f64 * octaves).round() as usize;
		// there must be at least two samples, one at min_freq and one at max_freq
		let samples = samples.max(2);
		SpectrumParams::exp_spaced(samples, self.min_log_freq.exp2(), self.max_log_freq.exp2())
	}

	fn build_spiral_config(&self) -> spiral::Config {
		spiral::Config {
			outer_pad: DEFAULT_SPIRAL_OUTER_PAD,
			center_pad: DEFAULT_SPIRAL_INNER_PAD,
			key_log_freq: self.key_log_freq
		}
	}
}

fn get_port_name<TM: TreeModelExt>(port_store: &TM, iter: &TreeIter) -> String {
	port_store
		.get_value(&iter, PORT_NAME_COL)
		.get::<String>()
		.expect("values in PORT_NAME_COL are strings")
		.expect("port names cannot be None")
}

fn build_source_type_selectors(controller: &Rc<RefCell<Controller>>) -> Vec<gtk::RadioButton> {
	let active_source_type = controller.borrow().get_source_type();

	let mut selectors = Vec::new();
	for source_type in [SourceType::Audio, SourceType::MIDI].iter() {
		let selector = if let Some(widget) = selectors.get(0) {
			gtk::RadioButton::with_label_from_widget(widget, &source_type.to_string())
		} else {
			gtk::RadioButton::with_label(&source_type.to_string())
		};

		selector.set_active(active_source_type == Some(*source_type));

		let controller_clone = controller.clone();
		selector.connect_toggled(
			move |selector| on_source_type_toggled(&controller_clone, selector, *source_type)
		);

		selectors.push(selector);
	}
	selectors
}