use futures::prelude::*;
use glib::Type;
use gtk::{prelude::*, TreeSelection, TreeIter};
use jack::{AudioOut, PortFlags, PortId, PortSpec};
use std::cell::RefCell;
use std::rc::Rc;

use crate::application::{Controller as AppController};
use crate::async_processor::AsyncProcessor;
use crate::error::Error;
use crate::graphic_renderer::GraphicRenderer;
use crate::gui::error_dialog;
use crate::note; // TODO: Rename this macro to not conflict with module.
use crate::note::Note;
use crate::pubsub::SubscriptionHandle;
use crate::spectrum::SpectrumParams;
use crate::source::{events::InputsChanged, SourceType};
use crate::spiral::{self, SpiralGenerator};

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

pub struct Controller {
	app_controller: Rc<RefCell<AppController>>,
	port_store: gtk::ListStore,
	min_log_freq: f64,
	max_log_freq: f64,
	key_log_freq: f64,
	samples_per_octave: usize,
	graphic_renderer: AsyncProcessor<GraphicRenderer>,
	inputs_changed_subscription: Option<SubscriptionHandle>,
	view_builder: gtk::Builder,
}

impl Controller {
	// TODO: Move this to an architecture overview or something.
	//
	// We have to be very careful about RefCells in async code. So borrowing a controller from a
	// RefCell then yielding with await is a big problem.
	fn update_spectrum_params(&self) -> impl Future<Output=Result<(), Error>> {
		let spectrum_params = self.build_spectrum_params();
		self.graphic_renderer
			.exec_cloned(move |renderer| {
				renderer.set_spectrum_params(spectrum_params);
			})
			.map_err(Error::Communication)
	}

	fn update_spiral_config(&self) -> impl Future<Output=Result<(), Error>> {
		let config = self.build_spiral_config();
		self.graphic_renderer
			.exec_cloned(move |renderer| {
				let spiral: &mut SpiralGenerator = renderer.generator_mut()
					.upcast_any_mut()
					.downcast_mut()
					.expect("update_spiral_config called when generator is not a SpiralGenerator");
				spiral.set_config(config);
			})
			.map_err(Error::Communication)
	}

	fn update_graphic_generator(&self) -> impl Future<Output=Result<(), Error>> {
		let generator = Box::new(SpiralGenerator::new(self.build_spiral_config()));
		self.graphic_renderer
			.exec_cloned(move |renderer| {
				renderer.set_generator(generator);
			})
			.map_err(Error::Communication)
	}

	fn refresh_inputs(&self) {
		let port_store = &self.port_store;

		let ports = self.app_controller.borrow()
			.jack_client()
			.map(|client| client.ports(None, Some(AudioOut.jack_port_type()), PortFlags::IS_OUTPUT))
			.unwrap_or_default();

		log::debug!("refreshing inputs: {:?}", ports);

		// Remove rows from ListStore.
		if let Some(iter) = port_store.get_iter_first() {
			loop {
				let found = ports.contains(&get_port_name(port_store, &iter));
				let iter_invalid = if !found {
					log::debug!("attempting to remove port");
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

pub fn new(app_controller: Rc<RefCell<AppController>>)
	-> Result<(Rc<RefCell<Controller>>, gtk::Box), Error>
{
	let column_types = [Type::String];
	let port_store = gtk::ListStore::new(&column_types[..]);

	let graphic_renderer = app_controller.borrow().graphic_renderer().clone();

	let view_builder = gtk::Builder::from_string(UI_DEF);

	let controller = Rc::new(RefCell::new(Controller {
		app_controller: app_controller.clone(),
		port_store,
		min_log_freq: DEFAULT_MIN_FREQ.log2(),
		max_log_freq: DEFAULT_MAX_FREQ.log2(),
		key_log_freq: DEFAULT_SPIRAL_KEY_FREQ.log2(),
		samples_per_octave: DEFAULT_SAMPLES_PER_OCTAVE,
		graphic_renderer,
		inputs_changed_subscription: None,
		view_builder,
	}));

	let view = init_view(&controller);

	// Update background processors with default control settings.
	{
		let controller = controller.borrow_mut();

		// Populate the initial port list.
		controller.refresh_inputs();

		let async_spectrum_params_update = controller.update_spectrum_params();
		let async_generator_update = controller.update_graphic_generator();
		let async_spiral_config_update = controller.update_spectrum_params();

		glib::MainContext::default().spawn_local(async move {
			// TODO: Handle errors better
			async_spectrum_params_update.await.unwrap();
			async_generator_update.await.unwrap();
			async_spiral_config_update.await.unwrap();
		});
	}

	Ok((controller, view))
}

fn init_view(controller: &Rc<RefCell<Controller>>) -> gtk::Box {
	let builder = controller.borrow().view_builder.clone();
	let view: gtk::Box = builder.get_object("control_pane").unwrap();
	let source_type_selection: gtk::Box = builder.get_object("source_type_selection").unwrap();
	let port_view: gtk::TreeView = builder.get_object("port_list").unwrap();
	let min_freq_scale: gtk::Scale = builder.get_object("min_freq_scale").unwrap();
	let max_freq_scale: gtk::Scale = builder.get_object("max_freq_scale").unwrap();
	let key_freq_scale: gtk::Scale = builder.get_object("key_freq_scale").unwrap();

	// Style the control pane.
	let style_provider = gtk::CssProvider::new();
	style_provider.load_from_data(STYLE).unwrap();

	// Populate source selection radio buttons.
	let app_controller = controller.borrow().app_controller.clone();
	for selector in build_source_type_selectors(&app_controller) {
		source_type_selection.add(&selector);
	}

	port_view.set_model(Some(&controller.borrow().port_store));

	// TODO: Maybe bound min/max frequency using window size.

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

	// Connect signal handler functions.
	let controller_clone = controller.clone();
	min_freq_scale.connect_change_value(
		move |_scale, _, value| on_min_freq_change(&controller_clone, value)
	);

	let controller_clone = controller.clone();
	max_freq_scale.connect_change_value(
		move |_scale, _, value| on_max_freq_change(&controller_clone, value)
	);

	let controller_clone = controller.clone();
	key_freq_scale.connect_change_value(
		move |_scale, _, value| on_key_freq_change(&controller_clone, value)
	);

	let selection = port_view.get_selection();
	let controller_clone = app_controller.clone();
	selection.connect_changed(move |selection| on_port_selected(&*controller_clone, selection));

	// Refresh port list when JACK inputs change.
	let controller_clone = controller.clone();
	let subscription = app_controller.borrow()
		.pubsub()
		.subscribe(move |notification: &InputsChanged| {
			on_input_ports_changed(&controller_clone, notification.clone());
			controller_clone.borrow().refresh_inputs()
		});

	{
		let mut controller = controller.borrow_mut();
		controller.inputs_changed_subscription = Some(subscription);

		// Set initial control values.
		min_freq_scale.set_value(controller.min_log_freq);
		max_freq_scale.set_value(controller.max_log_freq);
		key_freq_scale.set_value(controller.key_log_freq);
	}

	view
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

fn on_port_selected(app_controller: &RefCell<AppController>, selection: &TreeSelection) {
	let port_name = selection.get_selected()
		.map(|(port_store, iter)| get_port_name(&port_store, &iter));
	let mut app_controller = app_controller.borrow_mut();
	app_controller.connect_port(port_name);
}

fn on_source_type_toggled(
	app_controller: &RefCell<AppController>,
	selector: &gtk::RadioButton,
	source_type: SourceType
) {
	if !selector.get_active() {
		return;
	}

	let mut app_controller = app_controller.borrow_mut();
	if let Err(err) = app_controller.set_source_type(source_type) {
		log::error!("failed to change source type: {}", err);
	}
}

fn on_input_ports_changed(controller_ref: &Rc<RefCell<Controller>>, update: InputsChanged) {
	// TODO: Unfortunately, we need to poll until port_update is reflected.
	// https://github.com/jackaudio/jack2/issues/617
	let controller = controller_ref.clone();
	gtk::timeout_add(10, move || {
		if is_inputs_update_pending(&*controller.borrow(), update.clone()) {
			glib::Continue(true)
		} else {
			controller.borrow().refresh_inputs();
			glib::Continue(false)
		}
	});
}

fn is_inputs_update_pending(controller: &Controller, update: InputsChanged) -> bool {
	controller
		.app_controller.borrow()
		.jack_client()
		.map(move |client| {
			match update {
				// https://github.com/jackaudio/jack2/issues/617
				InputsChanged::Unregistered(port_id) => {
					if let Some(port) = client.port_by_id(port_id) {
						match port.name() {
							Ok(name) =>
								client
									.ports(None, None, PortFlags::empty())
									.contains(&name),
							Err(err) => {
								log::warn!("JACK port {} has no name", port_id);
								// Whatever, let's just say it's updated.
								false
							}
						}
					} else {
						false
					}
				}
				// I don't think we need to double-check any other cases.
				_ => false,
			}
		})
		.unwrap_or(false)
}

fn on_min_freq_change(controller_ref: &Rc<RefCell<Controller>>, value: f64) -> Inhibit {
	let mut controller = controller_ref.borrow_mut();
	if value > controller.max_log_freq {
		return Inhibit(true);
	}

	controller.min_log_freq = value;
	let async_update = controller.update_spectrum_params();

	let main_context = glib::MainContext::default();
	main_context.spawn_local(async move {
		match async_update.await {
			Ok(()) => {}
			Err(err) => error_dialog(err),
		}
	});

	Inhibit(false)
}

fn on_max_freq_change(controller_ref: &Rc<RefCell<Controller>>, value: f64) -> Inhibit {
	let mut controller = controller_ref.borrow_mut();
	if value < controller.min_log_freq {
		return Inhibit(true);
	}

	controller.max_log_freq = value;
	let async_update = controller.update_spectrum_params();

	let main_context = glib::MainContext::default();
	main_context.spawn_local(async move {
		match async_update.await {
			Ok(()) => {}
			Err(err) => error_dialog(err),
		}
	});

	Inhibit(false)
}

fn on_key_freq_change(controller_ref: &Rc<RefCell<Controller>>, value: f64) -> Inhibit {
	let mut controller = controller_ref.borrow_mut();

	controller.key_log_freq = value;
	let async_update = controller.update_spiral_config();

	let main_context = glib::MainContext::default();
	main_context.spawn_local(async move {
		match async_update.await {
			Ok(()) => {}
			Err(err) => error_dialog(err),
		}
	});

	Inhibit(false)
}

fn get_port_name<TM: TreeModelExt>(port_store: &TM, iter: &TreeIter) -> String {
	port_store
		.get_value(&iter, PORT_NAME_COL)
		.get::<String>()
		.expect("values in PORT_NAME_COL are strings")
		.expect("port names cannot be None")
}

fn build_source_type_selectors(app_controller: &Rc<RefCell<AppController>>)
	-> Vec<gtk::RadioButton>
{
	let active_source_type = app_controller.borrow().get_source_type();

	let mut selectors = Vec::new();
	for source_type in [SourceType::Audio, SourceType::MIDI].iter() {
		let selector = if let Some(widget) = selectors.get(0) {
			gtk::RadioButton::with_label_from_widget(widget, &source_type.to_string())
		} else {
			gtk::RadioButton::with_label(&source_type.to_string())
		};

		selector.set_active(active_source_type == Some(*source_type));

		let controller_clone = app_controller.clone();
		selector.connect_toggled(
			move |selector| on_source_type_toggled(&controller_clone, selector, *source_type)
		);

		selectors.push(selector);
	}
	selectors
}
