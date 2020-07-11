use glib::Type;
use gtk::{Orientation, TreeSelection, TreeIter};
use gtk::prelude::*;
use log::{debug, error};
use std::cell::RefCell;
use std::rc::Rc;

use crate::note;
use crate::application::Controller;
use crate::error::Error;
use crate::note::Note;
use crate::spectrum::SpectrumParams;
use crate::source::{events::InputsChanged, SourceType};
use jack::{PortFlags, AudioOut, PortSpec};

const STYLE: &[u8] = include_bytes!("control_pane.css");
const UI_DEF: &str = include_str!("control_pane.ui");

const PORT_NAME_COL: i32 = 0;

const MIN_NOTE: Note = note!(A, 0);
const MAX_NOTE: Note = note!(C, 8);

const DEFAULT_MIN_FREQ: f64 = 200.0; // Hz
const DEFAULT_MAX_FREQ: f64 = 2000.0; // Hz
const DEFAULT_SAMPLES_PER_OCTAVE: usize = 180;

pub struct ControlPane {
	app_controller: Rc<RefCell<Controller>>,
	local_controller: Rc<RefCell<ControlPaneController>>,
	view: gtk::Box,
}

impl ControlPane {
	pub fn new(app_controller: Rc<RefCell<Controller>>) -> Result<Self, Error> {
		let local_controller = Rc::new(RefCell::new(ControlPaneController::new()));

		let builder = gtk::Builder::from_string(UI_DEF);
		let view: gtk::Box = builder.get_object("control_pane").unwrap();
		let source_type_selection: gtk::Box = builder.get_object("source_type_selection").unwrap();
		let port_view: gtk::TreeView = builder.get_object("port_list").unwrap();
		let min_freq_scale: gtk::Scale = builder.get_object("min_freq_scale").unwrap();
		let max_freq_scale: gtk::Scale = builder.get_object("max_freq_scale").unwrap();

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

		{
			let controller = local_controller.borrow();
			min_freq_scale.set_value(controller.min_log_freq);
			max_freq_scale.set_value(controller.max_log_freq);
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

	pub fn new_old(app_controller: Rc<RefCell<Controller>>) -> Result<Self, Error> {
		let local_controller = Rc::new(RefCell::new(ControlPaneController::new()));

		let view = gtk::Box::new(Orientation::Vertical, 10);

		let style_provider = gtk::CssProvider::new();
		style_provider.load_from_data(STYLE)
			.map_err(Error::Glib)?;

		let style_ctx = view.get_style_context();
		style_ctx.add_provider(&style_provider, 1);
		style_ctx.add_class("control-pane");

		let source_control = gtk::Frame::new(Some("Input Source"));
		view.add(&source_control);

		let source_control_inner = gtk::Box::new(Orientation::Vertical, 10);
		source_control.add(&source_control_inner);

		let source_type_box = gtk::Box::new(Orientation::Horizontal, 0);
		source_control_inner.add(&source_type_box);

		for selector in build_source_type_selectors(&app_controller) {
			source_type_box.add(&selector);
		}

		let port_view = build_port_view(&local_controller.borrow().port_store());
		port_view.show();
		source_control_inner.add(&port_view);

		let selection = port_view.get_selection();
		let controller_clone = app_controller.clone();
		selection.connect_changed(move |selection| on_port_selected(&controller_clone, selection));

		let app_controller_clone = app_controller.clone();
		let local_controller_clone = local_controller.clone();
		app_controller.borrow()
			.pubsub()
			.subscribe(move |_: &InputsChanged| {
				local_controller_clone.borrow()
					.refresh_inputs(&app_controller_clone.borrow());
			});

		local_controller.borrow()
			.refresh_inputs(&app_controller.borrow());

		let min_freq_scale = gtk::ScaleBuilder::new()
			.adjustment(&gtk::Adjustment::new(
				5.0,
				0.0,
				(MAX_NOTE - MIN_NOTE + 1) as f64,
				1.0,
				0.0,
				0.0
			))
			.draw_value(false)
			.show_fill_level(false)
			.build();

		view.add(&min_freq_scale);

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
		error!("failed to change source type: {}", err);
	}
}

struct ControlPaneController {
	port_store: gtk::ListStore,
	min_log_freq: f64,
	max_log_freq: f64,
	samples_per_octave: usize,
}

impl ControlPaneController {
	fn new() -> Self {
		let column_types = [Type::String];
		let port_store = gtk::ListStore::new(&column_types[..]);
		ControlPaneController {
			port_store,
			min_log_freq: DEFAULT_MIN_FREQ.log2(),
			max_log_freq: DEFAULT_MAX_FREQ.log2(),
			samples_per_octave: DEFAULT_SAMPLES_PER_OCTAVE,
		}
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
}

fn get_port_name<TM: TreeModelExt>(port_store: &TM, iter: &TreeIter) -> String {
	port_store
		.get_value(&iter, PORT_NAME_COL)
		.get::<String>()
		.expect("values in PORT_NAME_COL are strings")
		.expect("port names cannot be None")
}

fn generate_spectrum_params(min_freq: f64, max_freq: f64, samples_per_octave: usize)
	-> SpectrumParams
{
	let octaves = max_freq.log2() - min_freq.log2();
	let samples = (samples_per_octave as f64 * octaves).round() as usize;
	// there must be at least two samples, one at min_freq and one at max_freq
	let samples = samples.max(2);
	SpectrumParams::exp_spaced(samples, min_freq, max_freq)
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