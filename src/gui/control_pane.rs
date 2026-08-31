// The port list uses GtkTreeView/GtkListStore, deprecated in GTK 4 in favour of
// GtkColumnView. Keeping them is an intentional, scoped decision; migrating to
// ColumnView is tracked as separate future work.
#![allow(deprecated)]

use gtk::{TreeIter, TreeSelection, prelude::*};
use std::{cell::RefCell, collections::HashMap, rc::Rc, sync::LazyLock};

use crate::app::config::{
	GraphicGeneratorConfig, SpectrumGeneratorConfig, SpectrumTransformConfig,
};
use crate::controllers::{
	AppController, ControlPaneController, DecibelConverterController, DiffuserController,
	VolumeNormalizerController,
	app::events::{InsertSpectrumTransform, SourcePortChanged},
	control_pane::PORT_NAME_COL,
};
use crate::error::Error;
use crate::gui::{controls, error_dialog, handle_async_err};
use crate::note; // TODO: Rename this macro to not conflict with module.
use crate::note::Note;
use crate::source::SourceType;
use crate::spectrum::transforms::{diffuser, volume_normalizer};

const UI_DEF: &str = include_str!("control_pane.ui.xml");

const MIN_NOTE: Note = note!(A, 0);
const MAX_NOTE: Note = note!(C, 8);

// Configure
// - Audio Source (Audio or MIDI & Port)
// - Spectrum Analysis
//   Spectrum Transform
// - Visualization

// Ideas: Maybe have a StatusBar at the box for async updates.

pub fn new(
	controller: &Rc<RefCell<ControlPaneController>>,
) -> Result<impl IsA<gtk::Widget> + use<>, Error> {
	let builder = gtk::Builder::from_string(UI_DEF);
	let view: gtk::Box = builder.object("control_pane").unwrap();
	let source_type_selection: gtk::Box = builder.object("source_type_selection").unwrap();
	let port_view: gtk::TreeView = builder.object("port_list").unwrap();
	let min_freq_scale: gtk::Scale = builder.object("min_freq_scale").unwrap();
	let max_freq_scale: gtk::Scale = builder.object("max_freq_scale").unwrap();
	let key_freq_scale: gtk::Scale = builder.object("key_freq_scale").unwrap();
	let add_transform_type_selector: gtk::ComboBoxText =
		builder.object("add_transform_type_selector").unwrap();

	let app_controller = controller.borrow().app_controller().clone();
	init_menu(&app_controller, &builder)?;

	// Transform type selector options.
	for (id, config) in get_transform_type_map().iter() {
		add_transform_type_selector.append(Some(id), get_spectrum_transform_name(config));
	}
	let app_controller_clone = app_controller.clone();
	add_transform_type_selector.connect_changed(move |selector| {
		if let Some(id) = selector.active_id() {
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
		source_type_selection.append(&selector);
	}

	port_view.set_model(Some(controller.borrow().port_store()));

	// TODO: Maybe bound min/max frequency using window size.

	min_freq_scale.set_adjustment(&gtk::Adjustment::new(
		MIN_NOTE.log_frequency(),
		MIN_NOTE.log_frequency(),
		MAX_NOTE.log_frequency(),
		1.0 / 12.0,
		0.0,
		0.0,
	));
	max_freq_scale.set_adjustment(&gtk::Adjustment::new(
		MIN_NOTE.log_frequency(),
		MIN_NOTE.log_frequency(),
		MAX_NOTE.log_frequency(),
		1.0 / 12.0,
		0.0,
		0.0,
	));
	key_freq_scale.set_adjustment(&gtk::Adjustment::new(
		note!(C, 3).log_frequency(),
		note!(C, 3).log_frequency(),
		note!(C, 4).log_frequency(),
		1.0 / 12.0,
		0.0,
		0.0,
	));

	// Connect signal handler functions.
	let controller_clone = controller.clone();
	min_freq_scale
		.connect_change_value(move |_scale, _, value| on_min_freq_change(&controller_clone, value));

	let controller_clone = controller.clone();
	max_freq_scale
		.connect_change_value(move |_scale, _, value| on_max_freq_change(&controller_clone, value));

	let controller_clone = controller.clone();
	key_freq_scale
		.connect_change_value(move |_scale, _, value| on_key_freq_change(&controller_clone, value));

	let selection = port_view.selection();
	let controller_clone = app_controller.clone();
	selection.connect_changed(move |selection| on_port_selected(&controller_clone, selection));

	{
		// Set initial control values.
		let app_controller = app_controller.borrow();
		min_freq_scale.set_value(app_controller.config.min_freq.log2());
		max_freq_scale.set_value(app_controller.config.max_freq.log2());
		// TODO:
		// key_freq_scale.set_value(app_controller.config.key_freq.log2());
	}

	Ok(view)
}

fn init_menu(
	app_controller_ref: &Rc<RefCell<AppController>>,
	builder: &gtk::Builder,
) -> Result<(), Error> {
	let menu: gtk::ListBox = builder.object("control_menu").unwrap();

	let control_stack: gtk::Stack = builder.object("control_stack").unwrap();
	let source_control: gtk::Frame = builder.object("source_control").unwrap();
	let spectrum_generator_control: gtk::Frame =
		builder.object("spectrum_generator_control").unwrap();
	let visualization_control: gtk::Frame = builder.object("visualization_control").unwrap();
	let add_transform_control: gtk::Frame = builder.object("add_transform_control").unwrap();

	let source_row: gtk::ListBoxRow = builder.object("source_row").unwrap();
	let spectrum_generator_row: gtk::ListBoxRow = builder.object("spectrum_generator_row").unwrap();
	let visualization_row: gtk::ListBoxRow = builder.object("visualization_row").unwrap();
	let add_transform_row: gtk::ListBoxRow = builder.object("add_transform_row").unwrap();

	let source_name: gtk::Label = builder.object("source_name").unwrap();
	let spectrum_generator_name: gtk::Label = builder.object("spectrum_generator_name").unwrap();
	let visualization_name: gtk::Label = builder.object("visualization_name").unwrap();

	// Initialize menu labels.
	let app_controller = app_controller_ref.borrow();
	source_name.set_label(get_source_name(&app_controller));
	spectrum_generator_name.set_label(get_spectrum_generator_name(&app_controller));
	visualization_name.set_label(get_visualization_name(&app_controller));

	// Initialize transform rows.
	for index in 0..app_controller.config.spectrum_transform_order.len() {
		let (id, config) = app_controller
			.config
			.spectrum_transform_by_index(index)?
			.expect("index is in range of spectrum_transform_order, so Ok result must be Some");
		let new_row = build_transform_row(get_spectrum_transform_name(config));
		let new_control = build_transform_control(id, config, app_controller_ref)?;
		menu.insert(&new_row, 2 + index as i32);
		control_stack.add_named(
			&new_control,
			Some(get_spectrum_transform_row_name(id).as_str()),
		);
	}

	// Subscribe to update menu labels on updates.
	let app_controller_clone = app_controller_ref.clone();
	let source_name_clone = source_name.clone();
	let source_name_subscription =
		app_controller
			.pubsub()
			.subscribe(move |_: &SourcePortChanged| {
				let app_controller = app_controller_clone.borrow();
				source_name_clone.set_label(get_source_name(&app_controller));
			});

	let app_controller_clone = app_controller_ref.clone();
	let add_transform_row_clone = add_transform_row.clone();
	let add_transform_control_clone = add_transform_control.clone();
	let control_stack_clone = control_stack.clone();
	menu.connect_row_activated(move |_, row| {
		on_control_row_activated(
			&app_controller_clone.borrow(),
			row,
			&control_stack_clone,
			&source_row,
			&spectrum_generator_row,
			&visualization_row,
			&add_transform_row_clone,
			&source_control,
			&spectrum_generator_control,
			&visualization_control,
			&add_transform_control_clone,
		);
	});

	let app_controller_clone = app_controller_ref.clone();
	let menu_clone = menu.clone();
	let insert_transform_subscription =
		app_controller
			.pubsub()
			.subscribe(move |notification: &InsertSpectrumTransform| {
				let InsertSpectrumTransform { index } = notification.clone();
				let result = on_insert_spectrum_transform(
					&app_controller_clone,
					&menu_clone,
					&control_stack,
					&add_transform_row,
					index,
				);
				if let Err(err) = result {
					log::error!("{}", err);
				}
			});

	// Keep subscriptions alive until view is destroyed.
	menu.connect_destroy(move |_| {
		let _ = &source_name_subscription;
		let _ = &insert_transform_subscription;
	});

	Ok(())
}

fn on_insert_spectrum_transform(
	app_controller_ref: &Rc<RefCell<AppController>>,
	menu: &gtk::ListBox,
	control_stack: &gtk::Stack,
	add_transform_row: &gtk::ListBoxRow,
	index: usize,
) -> Result<(), Error> {
	// TODO: Make this less brittle
	let app_controller = app_controller_ref.borrow();
	if let Some((id, config)) = app_controller.config.spectrum_transform_by_index(index)? {
		let new_row = build_transform_row(get_spectrum_transform_name(config));
		let new_control = build_transform_control(id, config, app_controller_ref)?;
		menu.insert(&new_row, 2 + index as i32);
		control_stack.add_named(
			&new_control,
			Some(get_spectrum_transform_row_name(id).as_str()),
		);

		if menu.selected_row().as_ref() == Some(add_transform_row) {
			menu.select_row(Some(&new_row));
		}
	}
	Ok(())
}

fn on_control_row_activated(
	app_controller: &AppController,
	row: &gtk::ListBoxRow,
	control_stack: &gtk::Stack,
	source_row: &gtk::ListBoxRow,
	spectrum_generator_row: &gtk::ListBoxRow,
	visualization_row: &gtk::ListBoxRow,
	add_transform_row_clone: &gtk::ListBoxRow,
	source_control: &gtk::Frame,
	spectrum_generator_control: &gtk::Frame,
	visualization_control: &gtk::Frame,
	add_transform_control_clone: &gtk::Frame,
) {
	let transform_count = app_controller.config.spectrum_transform_order.len();

	let row_index = row.index();
	assert!(
		row_index >= 0,
		"row was activated, so it must have an index"
	);
	let row_index = row_index as usize;

	if row_index == 0 {
		assert_eq!(row, source_row);
		control_stack.set_visible_child(source_control);
	} else if row_index == 1 {
		assert_eq!(row, spectrum_generator_row);
		control_stack.set_visible_child(spectrum_generator_control);
	} else if row_index < 2 + transform_count {
		let transform_id = app_controller.config.spectrum_transform_order[row_index - 2];
		let row_name = get_spectrum_transform_row_name(transform_id);
		if let Some(child) = control_stack.child_by_name(&row_name) {
			control_stack.set_visible_child(&child);
		} else {
			log::error!("control stack children out of sync with transforms");
		}
	} else if row_index == 2 + transform_count {
		assert_eq!(row, add_transform_row_clone);
		control_stack.set_visible_child(add_transform_control_clone);
	} else if row_index == 3 + transform_count {
		assert_eq!(row, visualization_row);
		control_stack.set_visible_child(visualization_control);
	} else {
		log::error!("unknown control menu row activated: index = {}", row_index);
	}
}

fn build_transform_row(name: &str) -> gtk::ListBoxRow {
	let row = gtk::ListBoxRow::new();

	let grid = gtk::Grid::builder()
		.row_homogeneous(true)
		.column_homogeneous(true)
		.build();
	row.set_child(Some(&grid));

	let button_box = gtk::Box::builder()
		.orientation(gtk::Orientation::Horizontal)
		.halign(gtk::Align::Center)
		.build();
	grid.attach(&button_box, 0, 0, 1, 1);

	let up_button = gtk::Button::builder().icon_name("go-up-symbolic").build();
	let down_button = gtk::Button::builder().icon_name("go-down-symbolic").build();
	let remove_button = gtk::Button::builder()
		.icon_name("list-remove-symbolic")
		.build();
	button_box.append(&down_button);
	button_box.append(&up_button);
	button_box.append(&remove_button);

	let label = gtk::Label::builder().label(name).build();
	grid.attach(&label, 1, 0, 1, 1);

	row
}

fn build_transform_control(
	id: u64,
	config: &SpectrumTransformConfig,
	app_controller: &Rc<RefCell<AppController>>,
) -> Result<impl IsA<gtk::Widget> + use<>, Error> {
	match config {
		SpectrumTransformConfig::DecibelConverter(_) => {
			let controller = DecibelConverterController::new(id, app_controller.clone());
			controls::decibel_converter::new(&controller).map(|widget| widget.upcast())
		}
		SpectrumTransformConfig::Diffuser(_) => {
			let controller = DiffuserController::new(id, app_controller.clone());
			controls::diffuser::new(&controller).map(|widget| widget.upcast())
		}
		SpectrumTransformConfig::VolumeNormalizer(_) => {
			let controller = VolumeNormalizerController::new(id, app_controller.clone());
			controls::volume_normalizer::new(&controller).map(|widget| widget.upcast())
		}
	}
}

fn on_port_selected(app_controller: &RefCell<AppController>, selection: &TreeSelection) {
	let port_name = selection
		.selected()
		.map(|(port_store, iter)| get_port_name(&port_store, &iter));
	let mut app_controller = app_controller.borrow_mut();
	if let Err(err) = app_controller.connect_port(port_name) {
		error_dialog(err);
	}
}

fn get_port_name<TM: IsA<gtk::TreeModel>>(port_store: &TM, iter: &TreeIter) -> String {
	port_store
		.get_value(iter, PORT_NAME_COL)
		.get::<String>()
		.expect("values in PORT_NAME_COL are strings")
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

fn on_min_freq_change(
	controller_ref: &Rc<RefCell<ControlPaneController>>,
	value: f64,
) -> glib::Propagation {
	let controller = controller_ref.borrow_mut();
	let mut app_controller = controller.app_controller().borrow_mut();

	let freq = value.exp2();
	if freq > app_controller.config.max_freq {
		return glib::Propagation::Stop;
	}

	app_controller.config.min_freq = freq;
	handle_async_err(app_controller.update_spectrum_params());
	glib::Propagation::Proceed
}

fn on_max_freq_change(
	controller_ref: &Rc<RefCell<ControlPaneController>>,
	value: f64,
) -> glib::Propagation {
	let controller = controller_ref.borrow_mut();
	let mut app_controller = controller.app_controller().borrow_mut();

	let freq = value.exp2();
	if freq < app_controller.config.min_freq {
		return glib::Propagation::Stop;
	}

	app_controller.config.max_freq = freq;
	handle_async_err(app_controller.update_spectrum_params());
	glib::Propagation::Proceed
}

fn on_key_freq_change(
	controller_ref: &Rc<RefCell<ControlPaneController>>,
	value: f64,
) -> glib::Propagation {
	let async_update = {
		let controller = controller_ref.borrow_mut();
		let mut app_controller = controller.app_controller().borrow_mut();
		let GraphicGeneratorConfig::Spiral(config) = &mut app_controller.config.graphic_generator;
		config.key_log_freq = value.log2();
		app_controller.update_graphic_generator()
	};

	handle_async_err(async_update);
	glib::Propagation::Proceed
}

fn build_source_type_selectors(
	app_controller: &Rc<RefCell<AppController>>,
) -> Vec<gtk::CheckButton> {
	let active_source_type = app_controller.borrow().get_source_type();

	// GTK 4 removed RadioButton; a group of CheckButtons sharing a group behaves
	// as a radio group.
	let mut selectors: Vec<gtk::CheckButton> = Vec::new();
	for source_type in [SourceType::Audio, SourceType::MIDI].iter() {
		let selector = gtk::CheckButton::with_label(&source_type.to_string());
		if let Some(first) = selectors.first() {
			selector.set_group(Some(first));
		}

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

fn get_spectrum_transform_row_name(id: u64) -> String {
	format!("transform_{}", id)
}

fn get_spectrum_transform_name(config: &SpectrumTransformConfig) -> &str {
	match config {
		SpectrumTransformConfig::DecibelConverter(_) => "Decibel Converter",
		SpectrumTransformConfig::Diffuser(_) => "Diffuser",
		SpectrumTransformConfig::VolumeNormalizer(_) => "Volume Normalizer",
	}
}

fn get_visualization_name(app_controller: &AppController) -> &str {
	match app_controller.config.graphic_generator {
		GraphicGeneratorConfig::Spiral(_) => "Spiral",
	}
}

fn get_transform_type_map() -> &'static HashMap<&'static str, SpectrumTransformConfig> {
	static MAP: LazyLock<HashMap<&'static str, SpectrumTransformConfig>> = LazyLock::new(|| {
		vec![
			(
				"Diffuser",
				SpectrumTransformConfig::Diffuser(diffuser::Config { width: 1.0 / 24.0 }),
			),
			(
				"VolumeNormalizer",
				SpectrumTransformConfig::VolumeNormalizer(volume_normalizer::Config { rate: 0.1 }),
			),
		]
		.into_iter()
		.collect()
	});
	&MAP
}
