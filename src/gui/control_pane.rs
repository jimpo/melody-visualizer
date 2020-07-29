use futures::prelude::*;
use glib::Type;
use gtk::{prelude::*, TreeSelection, TreeIter, ListBoxExt, WidgetExt};
use jack::{AudioOut, PortFlags, PortId, PortSpec};
use std::cell::RefCell;
use std::rc::Rc;

use crate::application::{events::SourcePortChanged, Controller as AppController};
use crate::async_processor::AsyncProcessor;
use crate::app::config::{SpectrumGeneratorConfig, GraphicGeneratorConfig};
use crate::error::Error;
use crate::graphic_renderer::GraphicRenderer;
use crate::gui::error_dialog;
use crate::note; // TODO: Rename this macro to not conflict with module.
use crate::note::Note;
use crate::pubsub::SubscriptionHandle;
use crate::spectrum::SpectrumParams;
use crate::source::{events::InputsChanged, SourceType};
use crate::spiral::{self, SpiralGenerator};
use crate::volume_normalizer::VolumeNormalizer;

const UI_DEF: &str = include_str!("control_pane.ui");

const PORT_NAME_COL: i32 = 0;

const MIN_NOTE: Note = note!(A, 0);
const MAX_NOTE: Note = note!(C, 8);

const DEFAULT_MIN_FREQ: f64 = 200.0; // Hz
const DEFAULT_MAX_FREQ: f64 = 2000.0; // Hz
const DEFAULT_SAMPLES_PER_OCTAVE: usize = 180;

// Configure
// - Audio Source (Audio or MIDI & Port)
// - Spectrum Analysis
//   Spectrum Transform
// - Visualization

// Ideas: Maybe have a StatusBar at the box for async updates.

pub struct Controller {
	app_controller: Rc<RefCell<AppController>>,
	port_store: gtk::ListStore,
	min_log_freq: f64,
	max_log_freq: f64,
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

		let volume_normalizer_init = controller
			.app_controller.borrow()
			.spectrum_renderer()
			.exec_cloned(|renderer| {
				renderer
					.transforms_mut()
					.push(Box::new(VolumeNormalizer::new(0.1)));
			});

		glib::MainContext::default().spawn_local(async move {
			// TODO: Handle errors better
			async_spectrum_params_update.await.unwrap();
			volume_normalizer_init.await.unwrap();
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

	let app_controller = controller.borrow().app_controller.clone();
	init_menu(&app_controller, &builder);

	// Populate source selection radio buttons.
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
	}

	{
		// Set initial control values.
		let app_controller = app_controller.borrow();
		min_freq_scale.set_value(app_controller.config.min_freq.log2());
		max_freq_scale.set_value(app_controller.config.max_freq.log2());
		// key_freq_scale.set_value(app_controller.config.key_freq.log2());
	}

	view
}

fn init_menu(app_controller_ref: &Rc<RefCell<AppController>>, builder: &gtk::Builder) {
	let menu: gtk::ListBox = builder.get_object("control_menu").unwrap();

	let control_stack: gtk::Stack = builder.get_object("control_stack").unwrap();
	let source_control: gtk::Frame = builder.get_object("source_control").unwrap();
	let spectrum_generator_control: gtk::Frame =
		builder.get_object("spectrum_generator_control").unwrap();
	let visualization_control: gtk::Frame = builder.get_object("visualization_control").unwrap();
	let add_transform_control: gtk::Frame = builder.get_object("add_transform_control").unwrap();

	let source_row: gtk::ListBoxRow = builder.get_object("source_row").unwrap();
	let spectrum_generator_row: gtk::ListBoxRow =
		builder.get_object("spectrum_generator_row").unwrap();
	let visualization_row: gtk::ListBoxRow =
		builder.get_object("visualization_row").unwrap();
	let add_transform_row: gtk::ListBoxRow = builder.get_object("add_transform_row").unwrap();

	let source_name: gtk::Label = builder.get_object("source_name").unwrap();
	let spectrum_generator_name: gtk::Label =
		builder.get_object("spectrum_generator_name").unwrap();
	let visualization_name: gtk::Label = builder.get_object("visualization_name").unwrap();

	// Initialize menu labels.
	let app_controller = app_controller_ref.borrow();
	source_name.set_label(get_source_name(&*app_controller));
	spectrum_generator_name.set_label(get_spectrum_generator_name(&*app_controller));
	visualization_name.set_label(get_visualization_name(&*app_controller));

	// Subscribe to update menu labels on updates.
	let app_controller_clone = app_controller_ref.clone();
	let source_name_clone = source_name.clone();
	let source_name_subscription = app_controller
		.pubsub()
		.subscribe(move |_: &SourcePortChanged| {
			let app_controller = app_controller_clone.borrow();
			source_name_clone.set_label(get_source_name(&*app_controller));
		});

	menu.connect_row_activated(move |_, row| {
		let child = if row == &source_row {
			&source_control
		} else if row == &spectrum_generator_row {
			&spectrum_generator_control
		} else if row == &add_transform_row {
			&add_transform_control
		} else if row == &visualization_row {
			&visualization_control
		} else {
			log::error!("unknown control menu row activated");
			return;
		};
		control_stack.set_visible_child(child);
	});

	// Keep subscriptions alive until view is destroyed.
	menu.connect_destroy(move |_| {
		let _ = &source_name_subscription;
	});
}

fn build_transform_row(name: &str) -> gtk::ListBoxRow {
	let row = gtk::ListBoxRow::new();

	let grid = gtk::GridBuilder::new()
		.row_homogeneous(true)
		.column_homogeneous(true)
		.build();
	row.add(&grid);

	let button_box = gtk::ButtonBoxBuilder::new()
		.orientation(gtk::Orientation::Horizontal)
		.layout_style(gtk::ButtonBoxStyle::Center)
		.build();
	grid.attach(&button_box, 0, 0, 1, 1);

	let up_icon = gtk::Image::from_icon_name(Some("up"), gtk::IconSize::Button);
	let down_icon = gtk::Image::from_icon_name(Some("down"), gtk::IconSize::Button);
	let remove_icon = gtk::Image::from_icon_name(Some("remove"), gtk::IconSize::Button);

	let up_button = gtk::ButtonBuilder::new().image(&up_icon).build();
	let down_button = gtk::ButtonBuilder::new().image(&down_icon).build();
	let remove_button = gtk::ButtonBuilder::new().image(&remove_icon).build();
	button_box.add(&down_button);
	button_box.add(&up_button);
	button_box.add(&remove_button);

	let label = gtk::LabelBuilder::new().label(name).build();
	grid.attach(&label, 1, 0, 1, 1);

	row
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
	if let Err(err) = app_controller.connect_port(port_name) {
		error_dialog(err);
	}
}

// fn on_source_type_toggled(
// 	app_controller: &RefCell<AppController>,
// 	selector: &gtk::RadioButton,
// 	source_type: SourceType
// ) {
// 	if !selector.get_active() {
// 		return;
// 	}
//
// 	let mut app_controller = app_controller.borrow_mut();
// 	if let Err(err) = app_controller.set_source_type(source_type) {
// 		log::error!("failed to change source type: {}", err);
// 	}
// }

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
	let async_update = {
		let mut controller = controller_ref.borrow_mut();
		let mut app_controller = controller.app_controller.borrow_mut();
		match &mut app_controller.config.graphic_generator {
			GraphicGeneratorConfig::Spiral(config) => {
				config.key_log_freq = value.log2();
				app_controller.update_graphic_generator()
			}
			config => {
				log::error!(
					"spiral control signal fired when other graphic generator is configured: {:?}",
					config
				);
				return Inhibit(false);
			}
		}
	};

	glib::MainContext::default().spawn_local(async move {
		if let Err(err) = async_update.await {
			error_dialog(err);
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

		// let controller_clone = app_controller.clone();
		// selector.connect_toggled(
		// 	move |selector| on_source_type_toggled(&controller_clone, selector, *source_type)
		// );

		selectors.push(selector);
	}
	selectors
}

fn get_source_name(app_controller: &AppController) -> &str {
	app_controller.source_port_name().unwrap_or("None")
}

fn get_spectrum_generator_name(app_controller: &AppController) -> &str {
	match app_controller.config.spectrum_generator {
		SpectrumGeneratorConfig::Audio(_) => "Default Audio Analyzer",
	}
}

fn get_visualization_name(app_controller: &AppController) -> &str {
	match app_controller.config.graphic_generator {
		GraphicGeneratorConfig::Spiral(_) => "Spiral",
	}
}