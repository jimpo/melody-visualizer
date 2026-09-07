use gio::prelude::*;
use gtk::prelude::*;
use std::{cell::RefCell, rc::Rc};

use crate::app::config::{
	GraphicGeneratorConfig, SpectrumGeneratorConfig, SpectrumTransformConfig,
};
use crate::audio::source::events::ConnectionChanged;
use crate::audio::source::{PortName, SourceType};
use crate::controllers::{
	AppController, ControlPaneController, DecibelConverterController, DiffuserController,
	VolumeNormalizerController, app::events::ConfigChanged,
};
use crate::error::Error;
use crate::gui::controls::key_row::key_name;
use crate::gui::{controls, error_dialog};
use crate::note::Note;
use crate::pubsub::SubscriptionHandle;
use crate::spectrum::TransformId;

const UI_DEF: &str = include_str!("control_pane.ui.xml");

const SEMITONES_PER_OCTAVE: f64 = 12.0;

/// The gap between the dot, the name and the summary on a stage row.
const ROW_SPACING: i32 = 12;

/// The gap between the tick and the name on a port row.
const PORT_ROW_SPACING: i32 = 10;

/// The tick that marks the connected port.
const PORT_TICK_ICON: &str = "object-select-symbolic";
const PORT_TICK_SIZE: i32 = 14;

// Ideas: Maybe have a StatusBar at the box for async updates.

pub fn new(
	controller: &Rc<RefCell<ControlPaneController>>,
) -> Result<impl IsA<gtk::Widget> + use<>, Error> {
	let builder = gtk::Builder::from_string(UI_DEF);
	let source_type_selection: gtk::Box = builder.object("source_type_selection").unwrap();
	let port_list: gtk::ListBox = builder.object("port_list").unwrap();
	let spectrum_caption: gtk::Label = builder.object("spectrum_caption").unwrap();

	let app_controller = controller.borrow().app_controller().clone();

	// Populate source selection radio buttons.
	for selector in build_source_type_selectors(&app_controller) {
		source_type_selection.append(&selector);
	}

	let port_subscription = connect_port_list(controller, &port_list);

	spectrum_caption.set_label(&spectrum_caption_text(&app_controller.borrow()));

	build_accordion(&app_controller, &builder, vec![port_subscription])
}

/// Fill the port list from the controller's model, and keep it and the
/// source's connection in step both ways.
fn connect_port_list(
	controller: &Rc<RefCell<ControlPaneController>>,
	port_list: &gtk::ListBox,
) -> SubscriptionHandle {
	let ports = controller.borrow().ports().clone();
	let app_controller = controller.borrow().app_controller().clone();

	port_list.bind_model(Some(&ports), |item| build_port_row(item).upcast());

	let controller_clone = controller.clone();
	port_list.connect_row_selected(move |_list, row| {
		let controller = controller_clone.borrow();
		let port = row.and_then(|row| controller.port_at(row.index() as u32));
		on_port_selected(&controller.app_controller().borrow(), port);
	});

	// The connected port is what JACK reports, so the selection follows it
	// rather than the click: a connection made outside the app selects its row
	// too, and a port that JACK drops leaves nothing selected.
	select_connected_port(&app_controller.borrow(), &ports, port_list);
	let app_controller_clone = app_controller.clone();
	let port_list_clone = port_list.clone();
	ports.connect_items_changed(move |ports, _position, _removed, _added| {
		select_connected_port(&app_controller_clone.borrow(), ports, &port_list_clone);
	});
	let app_controller_clone = app_controller.clone();
	let port_list = port_list.clone();
	app_controller
		.borrow()
		.pubsub()
		.subscribe(move |_: &ConnectionChanged| {
			select_connected_port(&app_controller_clone.borrow(), &ports, &port_list);
		})
}

/// A port row: a tick, shown only while the row is selected, and the name.
fn build_port_row(item: &glib::Object) -> gtk::Box {
	let name = item
		.downcast_ref::<gtk::StringObject>()
		.map(|item| item.string())
		.unwrap_or_default();

	let tick = gtk::Image::builder()
		.icon_name(PORT_TICK_ICON)
		.pixel_size(PORT_TICK_SIZE)
		.build();
	let label = gtk::Label::builder().label(name).xalign(0.0).build();

	let row = gtk::Box::new(gtk::Orientation::Horizontal, PORT_ROW_SPACING);
	row.append(&tick);
	row.append(&label);
	row
}

/// Select the row of the port feeding the source, or nothing when none is.
fn select_connected_port(
	app_controller: &AppController,
	ports: &gtk::StringList,
	port_list: &gtk::ListBox,
) {
	let row = app_controller.connected_input().and_then(|connected| {
		(0..ports.n_items())
			.find(|&position| ports.string(position).as_deref() == Some(connected.as_str()))
			.and_then(|position| port_list.row_at_index(position as i32))
	});
	port_list.select_row(row.as_ref());
}

/// Builds the pane itself: one stage per step of the pipeline, stacked in the
/// order the audio flows through them.
fn build_accordion(
	app_controller_ref: &Rc<RefCell<AppController>>,
	builder: &gtk::Builder,
	mut subscriptions: Vec<SubscriptionHandle>,
) -> Result<impl IsA<gtk::Widget> + use<>, Error> {
	let app_controller = app_controller_ref.borrow();

	let mut stages = vec![
		build_stage(
			StageId::Source,
			"Source",
			&builder.object::<gtk::Widget>("source_body").unwrap(),
			Switchable::No,
		),
		build_stage(
			StageId::Spectrum,
			spectrum_generator_name(&app_controller),
			&builder.object::<gtk::Widget>("spectrum_body").unwrap(),
			Switchable::No,
		),
	];
	for (id, config) in app_controller.config.spectrum_transforms.iter() {
		let body = build_transform_control(*id, config, app_controller_ref)?;
		stages.push(build_stage(
			StageId::Transform(*id),
			spectrum_transform_name(config),
			&body,
			transform_switchable(config),
		));
	}
	stages.push(build_stage(
		StageId::Spiral,
		graphic_generator_name(&app_controller),
		&controls::spiral::new(app_controller_ref),
		Switchable::No,
	));

	let stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
	for stage in stages.iter() {
		stack.append(&stage.row);
		stack.append(&stage.revealer);
		stack.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
	}

	let stages = Rc::new(stages);
	connect_exclusive_open(&stages);
	refresh_summaries(&app_controller, &stages);

	// A stage row reports what its stage is set to, so it has to follow both the
	// config and the connection JACK reports separately from it.
	subscriptions.push(subscribe_to_summaries::<ConfigChanged>(
		app_controller_ref,
		&stages,
	));
	subscriptions.push(subscribe_to_summaries::<ConnectionChanged>(
		app_controller_ref,
		&stages,
	));

	// Six stages with a body open overflow a short window.
	let view = gtk::ScrolledWindow::builder()
		.name("control_pane")
		.hscrollbar_policy(gtk::PolicyType::Never)
		.propagate_natural_width(true)
		.child(&stack)
		.build();

	// Keep the stages and their subscriptions alive until the view is destroyed.
	view.connect_destroy(move |_| {
		let _ = &stages;
		let _ = &subscriptions;
	});

	Ok(view)
}

/// One stage of the pipeline: the row that opens it and the body beneath.
///
/// The row is the whole toggle, because the design carries no disclosure
/// arrow. Its background is what tells an open stage from a hovered one. A
/// transform that can be switched off carries a switch at the end of its row,
/// beside the toggle rather than inside it, since a button swallows the
/// clicks of anything it holds.
struct Stage {
	id: StageId,
	row: gtk::Box,
	toggle: gtk::ToggleButton,
	revealer: gtk::Revealer,
	summary: gtk::Label,
}

/// Whether a stage row carries the switch that enables the stage.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Switchable {
	Yes,
	No,
}

/// The CSS class on a row whose switch is off.
const DIMMED_CLASS: &str = "dimmed";
/// The CSS class on the row of the open stage.
const OPEN_CLASS: &str = "open";

/// Which step of the pipeline a stage stands for. It names the colour of the
/// row's dot and the config the row's summary reads.
#[derive(Clone, Copy)]
enum StageId {
	Source,
	Spectrum,
	Transform(TransformId),
	Spiral,
}

impl StageId {
	/// The class that paints the row's dot, from the stage-type colours in
	/// `style.css`.
	fn dot_class(&self) -> &'static str {
		match self {
			Self::Source => "source",
			Self::Spectrum => "analysis",
			Self::Transform(_) => "transform",
			Self::Spiral => "display",
		}
	}
}

/// Whether a transform's row carries a switch.
///
/// The decibel converter has none: without it the spectrum is linear in
/// power, which reads as a few spikes and nothing else, so it is not optional.
fn transform_switchable(config: &SpectrumTransformConfig) -> Switchable {
	match config {
		SpectrumTransformConfig::DecibelConverter(_) => Switchable::No,
		SpectrumTransformConfig::Diffuser(_) | SpectrumTransformConfig::VolumeNormalizer(_) => {
			Switchable::Yes
		}
	}
}

fn build_stage(
	id: StageId,
	name: &str,
	body: &impl IsA<gtk::Widget>,
	switchable: Switchable,
) -> Stage {
	let dot = gtk::Box::builder().valign(gtk::Align::Center).build();
	dot.add_css_class("stage-dot");
	dot.add_css_class(id.dot_class());

	let name_label = gtk::Label::builder()
		.label(name)
		.xalign(0.0)
		.hexpand(true)
		.build();
	name_label.add_css_class("stage-name");

	let summary = gtk::Label::builder().xalign(1.0).build();
	summary.add_css_class("stage-summary");

	let bar = gtk::Box::new(gtk::Orientation::Horizontal, ROW_SPACING);
	bar.append(&dot);
	bar.append(&name_label);
	bar.append(&summary);

	let toggle = gtk::ToggleButton::builder()
		.child(&bar)
		.hexpand(true)
		.build();
	toggle.add_css_class("stage-toggle");

	let row = gtk::Box::new(gtk::Orientation::Horizontal, ROW_SPACING);
	row.add_css_class("stage-row");
	row.append(&toggle);

	let revealer = gtk::Revealer::builder()
		.transition_type(gtk::RevealerTransitionType::SlideDown)
		.child(body)
		.build();

	if switchable == Switchable::Yes {
		add_switch(&row, body);
	}

	Stage {
		id,
		row,
		toggle,
		revealer,
		summary,
	}
}

/// The switch at the end of a stage row. It means *enabled*, not bypassed.
///
/// Off dims the row and the body, and the body stays reachable but
/// insensitive: what the stage is set to can be read without switching it
/// back on. The row's summary keeps saying the same thing either way. What
/// the switch changes about the pipeline is nothing yet: no stage of the
/// chain can be skipped.
fn add_switch(row: &gtk::Box, body: &impl IsA<gtk::Widget>) {
	let switch = gtk::Switch::builder()
		.active(true)
		.valign(gtk::Align::Center)
		.build();

	row.append(&switch);

	let row = row.clone();
	let body = body.clone();
	switch.connect_active_notify(move |switch| {
		let enabled = switch.is_active();
		body.set_sensitive(enabled);
		if enabled {
			row.remove_css_class(DIMMED_CLASS);
		} else {
			row.add_css_class(DIMMED_CLASS);
		}
	});
}

/// Keep at most one stage open. Clicking the open stage's bar closes it, which
/// leaves every body shut.
fn connect_exclusive_open(stages: &Rc<Vec<Stage>>) {
	for (index, stage) in stages.iter().enumerate() {
		// The closures hang off the toggles the stages own, so they hold the
		// stages weakly to keep the pane from leaking itself.
		let stages = Rc::downgrade(stages);
		stage.toggle.connect_toggled(move |toggle| {
			let Some(stages) = stages.upgrade() else {
				return;
			};
			let open = toggle.is_active();
			let stage = &stages[index];
			stage.revealer.set_reveal_child(open);
			if open {
				stage.row.add_css_class(OPEN_CLASS);
			} else {
				stage.row.remove_css_class(OPEN_CLASS);
			}
			if open {
				for (other_index, other) in stages.iter().enumerate() {
					if other_index != index {
						// Deactivating runs this handler again for that stage,
						// with `open` false, so it shuts its own body and stops
						// there.
						other.toggle.set_active(false);
					}
				}
			}
		});
	}
}

/// Refresh every row's summary whenever a notification of type `Event` arrives.
fn subscribe_to_summaries<Event: Send + 'static>(
	app_controller_ref: &Rc<RefCell<AppController>>,
	stages: &Rc<Vec<Stage>>,
) -> SubscriptionHandle {
	let app_controller_clone = app_controller_ref.clone();
	let stages = Rc::downgrade(stages);
	app_controller_ref
		.borrow()
		.pubsub()
		.subscribe(move |_: &Event| {
			if let Some(stages) = stages.upgrade() {
				refresh_summaries(&app_controller_clone.borrow(), &stages);
			}
		})
}

fn refresh_summaries(app_controller: &AppController, stages: &[Stage]) {
	for stage in stages {
		stage
			.summary
			.set_label(&stage_summary(app_controller, stage.id));
	}
}

/// What a collapsed row reports its stage is set to.
fn stage_summary(app_controller: &AppController, id: StageId) -> String {
	match id {
		StageId::Source => source_name(app_controller),
		StageId::Spectrum => format!(
			"1/{} tone",
			(app_controller.config.samples_per_octave as f64 / SEMITONES_PER_OCTAVE).round()
		),
		StageId::Transform(id) => transform_summary(app_controller, id),
		StageId::Spiral => spiral_summary(app_controller),
	}
}

fn transform_summary(app_controller: &AppController, id: TransformId) -> String {
	// The chain is fixed, so a row outlives every config the app can reach.
	let config = app_controller
		.config
		.spectrum_transform(id)
		.expect("a stage row is built from a transform of the fixed chain");
	match config {
		SpectrumTransformConfig::DecibelConverter(config) => {
			format!(
				"floor {}",
				controls::decibel_converter::floor_text(config.min_level)
			)
		}
		SpectrumTransformConfig::Diffuser(config) => controls::diffuser::width_text(config.width),
		SpectrumTransformConfig::VolumeNormalizer(config) => {
			format!(
				"rate {}",
				controls::volume_normalizer::rate_text(config.rate)
			)
		}
	}
}

fn spiral_summary(app_controller: &AppController) -> String {
	format!(
		"{}–{} · key {}",
		Note::nearest(app_controller.config.min_freq.log2()),
		Note::nearest(app_controller.config.max_freq.log2()),
		key_name(controls::spiral::key(app_controller).pitch_class),
	)
}

/// The grid the spectrum stage analyses on, in the terms its controls set.
fn spectrum_caption_text(app_controller: &AppController) -> String {
	let SpectrumGeneratorConfig::Audio(generator) = &app_controller.config.spectrum_generator;
	format!(
		"{} bins / octave · window {}",
		app_controller.config.samples_per_octave, generator.dft_window_size,
	)
}

fn build_transform_control(
	id: TransformId,
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

/// Feed the source from `port`, unless it already is.
///
/// The selection follows the connection as well as leading it, so a selection
/// that only echoes what JACK reports must not be sent back to JACK: connecting
/// a port that is already connected is an error there.
fn on_port_selected(app_controller: &AppController, port: Option<PortName>) {
	if port == app_controller.connected_input() {
		return;
	}
	if let Err(err) = app_controller.connect_port(port) {
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

/// The port feeding the source, as the pane and the header bar both report it.
pub fn source_name(app_controller: &AppController) -> String {
	app_controller
		.connected_input()
		.map_or_else(|| "None".to_string(), |port| port.0)
}

fn spectrum_generator_name(app_controller: &AppController) -> &'static str {
	match app_controller.config.spectrum_generator {
		SpectrumGeneratorConfig::Audio(_) => "Spectrum",
	}
}

fn spectrum_transform_name(config: &SpectrumTransformConfig) -> &'static str {
	match config {
		SpectrumTransformConfig::DecibelConverter(_) => "Decibel Converter",
		SpectrumTransformConfig::Diffuser(_) => "Diffuser",
		SpectrumTransformConfig::VolumeNormalizer(_) => "Volume Normalizer",
	}
}

fn graphic_generator_name(app_controller: &AppController) -> &'static str {
	match app_controller.config.graphic_generator {
		GraphicGeneratorConfig::Spiral(_) => "Spiral",
	}
}
