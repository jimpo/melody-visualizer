use futures::prelude::*;
use gtk::{prelude::*, TreeSelection, TreeIter, ListBoxExt, WidgetExt};
use lazy_static::lazy_static;
use std::{
	cell::RefCell,
	collections::HashMap,
	rc::Rc,
};

use crate::app::config::{GraphicGeneratorConfig, SpectrumGeneratorConfig, SpectrumTransformConfig};
use crate::controllers::{
	AppController, ControlPaneController,
	app::events::{SourcePortChanged, InsertSpectrumTransform},
	control_pane::PORT_NAME_COL,
};
use crate::error::Error;
use crate::gui::error_dialog;
use crate::note; // TODO: Rename this macro to not conflict with module.
use crate::note::Note;
use crate::source::SourceType;
use crate::volume_normalizer;

const UI_DEF: &str = include_str!("control_pane.ui");

const MIN_NOTE: Note = note!(A, 0);
const MAX_NOTE: Note = note!(C, 8);

// Configure
// - Audio Source (Audio or MIDI & Port)
// - Spectrum Analysis
//   Spectrum Transform
// - Visualization

// Ideas: Maybe have a StatusBar at the box for async updates.

pub fn new(controller: &Rc<RefCell<ControlPaneController>>) -> gtk::Box {
	let builder = gtk::Builder::from_string(UI_DEF);
	let view: gtk::Box = builder.get_object("control_pane").unwrap();
	let source_type_selection: gtk::Box = builder.get_object("source_type_selection").unwrap();
	let port_view: gtk::TreeView = builder.get_object("port_list").unwrap();
	let min_freq_scale: gtk::Scale = builder.get_object("min_freq_scale").unwrap();
	let max_freq_scale: gtk::Scale = builder.get_object("max_freq_scale").unwrap();
	let key_freq_scale: gtk::Scale = builder.get_object("key_freq_scale").unwrap();
	let add_transform_type_selector: gtk::ComboBoxText =
		builder.get_object("add_transform_type_selector").unwrap();

	let app_controller = controller.borrow().app_controller().clone();
	init_menu(&app_controller, &builder);

	// Transform type selector options.
	for (id, config) in get_transform_type_map().iter() {
		add_transform_type_selector.append(Some(id), get_spectrum_transform_name(config));
	}
	let app_controller_clone = app_controller.clone();
	add_transform_type_selector.connect_changed(move |selector| {
		if let Some(id) = selector.get_active_id() {
			selector.set_active_id(None);
			if let Some(config) = get_transform_type_map().get(id.as_str()) {
				let mut app_controller = app_controller_clone.borrow_mut();
				handle_async_err(app_controller.insert_spectrum_transform(config.clone()));
			} else {
				log::error!("unknown transform type selected: {}", id);
			}
		}
	});

	// Populate source selection radio buttons.
	for selector in build_source_type_selectors(&app_controller) {
		source_type_selection.add(&selector);
	}

	port_view.set_model(Some(controller.borrow().port_store()));

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

	// Initialize transform rows.
	for (index, config) in app_controller.config.spectrum_transforms.iter().enumerate() {
		let new_row = build_transform_row(get_spectrum_transform_name(config));
		menu.insert(&new_row, 2 + index as i32);
	}

	// Subscribe to update menu labels on updates.
	let app_controller_clone = app_controller_ref.clone();
	let source_name_clone = source_name.clone();
	let source_name_subscription = app_controller
		.pubsub()
		.subscribe(move |_: &SourcePortChanged| {
			let app_controller = app_controller_clone.borrow();
			source_name_clone.set_label(get_source_name(&*app_controller));
		});

	let add_transform_row_clone = add_transform_row.clone();
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


	let app_controller_clone = app_controller_ref.clone();
	let menu_clone = menu.clone();
	let insert_transform_subscription = app_controller
		.pubsub()
		.subscribe(move |notification: &InsertSpectrumTransform| {
			// TODO: Make this less brittle
			let InsertSpectrumTransform { index } = notification.clone();
			let app_controller = app_controller_clone.borrow();
			if let Some(ref config) = app_controller.config.spectrum_transforms.get(index) {
				let new_row = build_transform_row(get_spectrum_transform_name(config));
				menu_clone.insert(&new_row, 2 + index as i32);
				new_row.show_all();

				if menu_clone.get_selected_row() == Some(add_transform_row_clone.clone()) {
					menu_clone.select_row(Some(&new_row));
				}
			}
		});

	// Keep subscriptions alive until view is destroyed.
	menu.connect_destroy(move |_| {
		let _ = &source_name_subscription;
		let _ = &insert_transform_subscription;
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

fn on_port_selected(app_controller: &RefCell<AppController>, selection: &TreeSelection) {
	let port_name = selection.get_selected()
		.map(|(port_store, iter)| get_port_name(&port_store, &iter));
	let mut app_controller = app_controller.borrow_mut();
	if let Err(err) = app_controller.connect_port(port_name) {
		error_dialog(err);
	}
}

fn get_port_name<TM: TreeModelExt>(port_store: &TM, iter: &TreeIter) -> String {
	port_store
		.get_value(&iter, PORT_NAME_COL)
		.get::<String>()
		.expect("values in PORT_NAME_COL are strings")
		.expect("port names cannot be None")
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


fn on_min_freq_change(controller_ref: &Rc<RefCell<ControlPaneController>>, value: f64) -> Inhibit {
	let controller = controller_ref.borrow_mut();
	let mut app_controller = controller.app_controller().borrow_mut();

	let freq = value.exp2();
	if freq > app_controller.config.max_freq {
		return Inhibit(true);
	}

	app_controller.config.min_freq = freq;
	handle_async_err(app_controller.update_spectrum_params());
	Inhibit(false)
}

fn on_max_freq_change(controller_ref: &Rc<RefCell<ControlPaneController>>, value: f64) -> Inhibit {
	let controller = controller_ref.borrow_mut();
	let mut app_controller = controller.app_controller().borrow_mut();

	let freq = value.exp2();
	if freq < app_controller.config.min_freq {
		return Inhibit(true);
	}

	app_controller.config.max_freq = freq;
	handle_async_err(app_controller.update_spectrum_params());
	Inhibit(false)
}

fn on_key_freq_change(controller_ref: &Rc<RefCell<ControlPaneController>>, value: f64) -> Inhibit {
	let async_update = {
		let controller = controller_ref.borrow_mut();
		let mut app_controller = controller.app_controller().borrow_mut();
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

	handle_async_err(async_update);
	Inhibit(false)
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

fn get_spectrum_transform_name(config: &SpectrumTransformConfig) -> &str {
	match config {
		SpectrumTransformConfig::VolumeNormalizer(_) => "Volume Normalizer",
	}
}

fn get_visualization_name(app_controller: &AppController) -> &str {
	match app_controller.config.graphic_generator {
		GraphicGeneratorConfig::Spiral(_) => "Spiral",
	}
}

fn get_transform_type_map() -> &'static HashMap<&'static str, SpectrumTransformConfig> {
	lazy_static! {
    	static ref MAP: HashMap<&'static str, SpectrumTransformConfig> =
			vec![
				(
					"VolumeNormalizer",
					 SpectrumTransformConfig::VolumeNormalizer(
					 	volume_normalizer::Config { rate: 0.1 }
					 )
				),
			]
				.into_iter()
				.collect();
	}
	&*MAP
}

fn handle_async_err(fut: impl Future<Output=Result<(), Error>> + 'static) {
	glib::MainContext::default().spawn_local(async move {
		if let Err(err) = fut.await {
			error_dialog(err);
		}
	});
}