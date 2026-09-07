// The port list uses GtkTreeView/GtkListStore, deprecated in GTK 4 in favour of
// GtkColumnView. Keeping them is an intentional, scoped decision; migrating to
// ColumnView is tracked as separate future work.
#![allow(deprecated)]

use gtk::{TreeIter, TreeSelection, prelude::*};
use std::{cell::RefCell, rc::Rc};

use crate::app::config::{
	GraphicGeneratorConfig, SpectrumGeneratorConfig, SpectrumTransformConfig,
};
use crate::audio::source::events::ConnectionChanged;
use crate::audio::source::{PortName, SourceType};
use crate::controllers::{
	AppController, ControlPaneController, DecibelConverterController, DiffuserController,
	VolumeNormalizerController, app::events::ConfigChanged, control_pane::PORT_NAME_COL,
};
use crate::error::Error;
use crate::gui::{controls, error_dialog, handle_async_err};
use crate::note; // TODO: Rename this macro to not conflict with module.
use crate::note::Note;
use crate::pubsub::SubscriptionHandle;
use crate::spectrum::TransformId;

const UI_DEF: &str = include_str!("control_pane.ui.xml");

const MIN_NOTE: Note = note!(A, 0);
const MAX_NOTE: Note = note!(C, 8);

const SEMITONES_PER_OCTAVE: f64 = 12.0;

/// The gap between the dot, the name and the summary on a stage row.
const ROW_SPACING: i32 = 12;

// Ideas: Maybe have a StatusBar at the box for async updates.

pub fn new(
	controller: &Rc<RefCell<ControlPaneController>>,
) -> Result<impl IsA<gtk::Widget> + use<>, Error> {
	let builder = gtk::Builder::from_string(UI_DEF);
	let source_type_selection: gtk::Box = builder.object("source_type_selection").unwrap();
	let port_view: gtk::TreeView = builder.object("port_list").unwrap();
	let spectrum_caption: gtk::Label = builder.object("spectrum_caption").unwrap();
	let min_freq_scale: gtk::Scale = builder.object("min_freq_scale").unwrap();
	let max_freq_scale: gtk::Scale = builder.object("max_freq_scale").unwrap();
	let key_freq_scale: gtk::Scale = builder.object("key_freq_scale").unwrap();

	let app_controller = controller.borrow().app_controller().clone();

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
		spectrum_caption.set_label(&spectrum_caption_text(&app_controller));
		min_freq_scale.set_value(app_controller.config.min_freq.log2());
		max_freq_scale.set_value(app_controller.config.max_freq.log2());
		// TODO:
		// key_freq_scale.set_value(app_controller.config.key_freq.log2());
	}

	build_accordion(&app_controller, &builder)
}

/// Builds the pane itself: one stage per step of the pipeline, stacked in the
/// order the audio flows through them.
fn build_accordion(
	app_controller_ref: &Rc<RefCell<AppController>>,
	builder: &gtk::Builder,
) -> Result<impl IsA<gtk::Widget> + use<>, Error> {
	let app_controller = app_controller_ref.borrow();

	let mut stages = vec![
		build_stage(
			StageId::Source,
			"Source",
			&builder.object::<gtk::Widget>("source_body").unwrap(),
		),
		build_stage(
			StageId::Spectrum,
			spectrum_generator_name(&app_controller),
			&builder.object::<gtk::Widget>("spectrum_body").unwrap(),
		),
	];
	for (id, config) in app_controller.config.spectrum_transforms.iter() {
		let body = build_transform_control(*id, config, app_controller_ref)?;
		stages.push(build_stage(
			StageId::Transform(*id),
			spectrum_transform_name(config),
			&body,
		));
	}
	stages.push(build_stage(
		StageId::Spiral,
		graphic_generator_name(&app_controller),
		&builder.object::<gtk::Widget>("spiral_body").unwrap(),
	));

	let stack = gtk::Box::new(gtk::Orientation::Vertical, 0);
	for stage in stages.iter() {
		stack.append(&stage.toggle);
		stack.append(&stage.revealer);
		stack.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
	}

	let stages = Rc::new(stages);
	connect_exclusive_open(&stages);
	refresh_summaries(&app_controller, &stages);

	// A stage row reports what its stage is set to, so it has to follow both the
	// config and the connection JACK reports separately from it.
	let subscriptions = [
		subscribe_to_summaries::<ConfigChanged>(app_controller_ref, &stages),
		subscribe_to_summaries::<ConnectionChanged>(app_controller_ref, &stages),
	];

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

/// One stage of the pipeline: the bar that opens it and the body beneath.
///
/// The bar is the whole toggle, because the design carries no disclosure arrow.
/// Its background is what tells an open stage from a hovered one.
struct Stage {
	id: StageId,
	toggle: gtk::ToggleButton,
	revealer: gtk::Revealer,
	summary: gtk::Label,
}

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

fn build_stage(id: StageId, name: &str, body: &impl IsA<gtk::Widget>) -> Stage {
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

	let toggle = gtk::ToggleButton::builder().child(&bar).build();
	toggle.add_css_class("stage-row");

	let revealer = gtk::Revealer::builder()
		.transition_type(gtk::RevealerTransitionType::SlideDown)
		.child(body)
		.build();

	Stage {
		id,
		toggle,
		revealer,
		summary,
	}
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
			stages[index].revealer.set_reveal_child(open);
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
	let GraphicGeneratorConfig::Spiral(config) = &app_controller.config.graphic_generator;
	format!(
		"{}–{} · key {}",
		Note::nearest(app_controller.config.min_freq.log2()),
		Note::nearest(app_controller.config.max_freq.log2()),
		Note::nearest(config.key_log_freq).pitch_class,
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

fn on_port_selected(app_controller: &RefCell<AppController>, selection: &TreeSelection) {
	let port_name = selection
		.selected()
		.map(|(port_store, iter)| get_port_name(&port_store, &iter));
	if let Err(err) = app_controller.borrow().connect_port(port_name) {
		error_dialog(err);
	}
}

fn get_port_name<TM: IsA<gtk::TreeModel>>(port_store: &TM, iter: &TreeIter) -> PortName {
	port_store
		.get_value(iter, PORT_NAME_COL)
		.get::<String>()
		.expect("values in PORT_NAME_COL are strings")
		.into()
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
