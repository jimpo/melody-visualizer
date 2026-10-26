//! The control pane: one row per stage of the pipeline, stacked in the order
//! the audio flows through them. Clicking a row opens its controls beneath it,
//! and opening one stage closes the one that was open.
//!
//! Every control is a parameter, so the pane reads its values from the
//! parameters on every redraw and changes them only through the host.

use nice_plug::prelude::*;
use nice_plug_iced::iced::{
	Alignment, Background, Color, Element, Length, Theme, border,
	widget::{
		Column, Row, button, column, container, row, rule, scrollable, slider, space, text, toggler,
	},
};

use super::Message;
use super::range_slider::range_slider;
use crate::note::Note;
use crate::params::{KEYS, PluginParams};

/// The pane's width, in logical pixels.
pub const WIDTH: f32 = 420.0;

const BACKGROUND: Color = Color::from_rgb8(0x30, 0x30, 0x30);
const INK_DIM: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.55);
const HAIRLINE: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.08);
const ROW_HOVER: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.03);
/// An open row is brighter than a hovered one because the design carries no
/// disclosure arrow: the row's background is all that tells them apart.
const ROW_OPEN: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.10);

const WHITE_KEY: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.08);

/// A stage's type shows only in the colour of its row's dot.
const ANALYSIS: Color = Color::from_rgb8(0xc0, 0x61, 0xcb);
const TRANSFORM: Color = Color::from_rgb8(0x57, 0xe3, 0x89);
const DISPLAY: Color = Color::from_rgb8(0x62, 0xa0, 0xea);

const ROW_HEIGHT: f32 = 52.0;
/// The gap between two control groups in one stage's body.
const GROUP_SPACING: f32 = 22.0;
/// The gap between a group's label row, its control and its caption.
const CONTROL_SPACING: f32 = 8.0;

/// The stages, in the order the audio flows through them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
	Spectrum,
	HarmonicSummation,
	PowerMap,
	Diffuser,
	VolumeNormalizer,
	Spiral,
}

pub fn view<'a>(params: &'a PluginParams, open: Option<Stage>) -> Element<'a, Message> {
	let stages = [
		Stage::Spectrum,
		Stage::HarmonicSummation,
		Stage::PowerMap,
		Stage::Diffuser,
		Stage::VolumeNormalizer,
		Stage::Spiral,
	];
	let mut pane = Column::new();
	for stage in stages {
		let is_open = open == Some(stage);
		pane = pane.push(stage_row(params, stage, is_open));
		if is_open {
			pane = pane.push(container(body(params, stage)).padding([14.0, 16.0]));
		}
		pane = pane.push(rule::horizontal(1).style(|_| rule::Style {
			color: HAIRLINE,
			radius: 0.0.into(),
			fill_mode: rule::FillMode::Full,
			snap: true,
		}));
	}
	container(scrollable(pane))
		.width(WIDTH)
		.height(Length::Fill)
		.style(|_| container::background(BACKGROUND))
		.into()
}

/// A stage's row: its dot, its name and what it is set to, and the switch at
/// the end for a stage that can be switched off. The row is the whole toggle
/// that opens the stage, apart from the switch.
fn stage_row<'a>(params: &'a PluginParams, stage: Stage, open: bool) -> Element<'a, Message> {
	let switch = switch_param(params, stage);
	let enabled = switch.is_none_or(|param| param.value());
	let (name, colour, summary) = match stage {
		Stage::Spectrum => (
			"Spectrum",
			ANALYSIS,
			format!("{} · {}", params.window_ms, params.update_rate),
		),
		Stage::HarmonicSummation => (
			"Harmonic Summation",
			TRANSFORM,
			format!("{} · {}", params.harmonics, params.harmonic_decay),
		),
		Stage::PowerMap => (
			"Power Map",
			TRANSFORM,
			format!("exponent {}", params.exponent),
		),
		Stage::Diffuser => ("Diffuser", TRANSFORM, params.diffuser_width.to_string()),
		Stage::VolumeNormalizer => (
			"Volume Normalizer",
			TRANSFORM,
			format!("rate {}", params.normalizer_rate),
		),
		Stage::Spiral => (
			"Spiral",
			DISPLAY,
			format!(
				"{}–{} · key {}",
				params.lowest_note, params.highest_note, params.key
			),
		),
	};

	// A stage switched off dims, and its summary still says what it is set to.
	let (dot_alpha, name_alpha, summary_alpha) = if enabled {
		(1.0, 1.0, INK_DIM.a)
	} else {
		(0.35, 0.4, 0.28)
	};
	let dot = container(space())
		.width(8)
		.height(8)
		.style(move |_| container::Style {
			background: Some(Background::Color(Color {
				a: dot_alpha,
				..colour
			})),
			border: border::rounded(4),
			..container::Style::default()
		});
	let label = row![
		dot,
		text(name)
			.size(15)
			.wrapping(text::Wrapping::None)
			.color(Color {
				a: name_alpha,
				..Color::WHITE
			}),
		space::horizontal(),
		text(summary)
			.size(13)
			.wrapping(text::Wrapping::None)
			.color(Color {
				a: summary_alpha,
				..Color::WHITE
			}),
	]
	.spacing(12)
	.align_y(Alignment::Center);
	// Clipped, so a summary too long for the row is cut short at the row's
	// edge rather than drawn under the switch.
	let toggle = button(
		container(label)
			.height(Length::Fill)
			.center_y(Length::Fill)
			.clip(true),
	)
	.on_press(Message::Open(stage))
	.width(Length::Fill)
	.height(ROW_HEIGHT)
	.padding(0)
	.style(|_, status| button::Style {
		background: matches!(status, button::Status::Hovered | button::Status::Pressed)
			.then_some(Background::Color(ROW_HOVER)),
		text_color: Color::WHITE,
		..button::Style::default()
	});

	let mut bar = row![toggle].spacing(12).align_y(Alignment::Center);
	if let Some(param) = switch {
		bar = bar.push(
			toggler(param.value())
				.on_toggle(move |on| Message::Click(param.as_ptr(), if on { 1.0 } else { 0.0 }))
				.size(24)
				.style(switch_style),
		);
	}
	container(bar)
		.padding([0.0, 16.0])
		.width(Length::Fill)
		.style(move |_| container::background(if open { ROW_OPEN } else { Color::TRANSPARENT }))
		.into()
}

/// The parameter behind a stage's switch, for a stage that has one.
fn switch_param(params: &PluginParams, stage: Stage) -> Option<&BoolParam> {
	match stage {
		Stage::HarmonicSummation => Some(&params.harmonic_summation),
		Stage::PowerMap => Some(&params.power_map),
		Stage::Diffuser => Some(&params.diffuser),
		_ => None,
	}
}

fn body<'a>(params: &'a PluginParams, stage: Stage) -> Element<'a, Message> {
	let groups: Vec<Element<'a, Message>> = match stage {
		Stage::Spectrum => vec![
			param_slider(
				&params.window_ms,
				"Window",
				"The audio each spectrum analyses. Longer separates pitches better and reacts later.",
			),
			param_slider(
				&params.update_rate,
				"Update rate",
				"How often a new spectrum is produced, whatever the window.",
			),
		],
		Stage::HarmonicSummation => vec![
			param_slider(
				&params.harmonics,
				"Harmonics",
				"How many multiples of its frequency each partial collects, itself included.",
			),
			param_slider(
				&params.harmonic_decay,
				"Decay",
				"The weight of each harmonic against the one below it.",
			),
		],
		Stage::PowerMap => vec![param_slider(
			&params.exponent,
			"Exponent",
			"Higher dims everything quieter than the loudest partial.",
		)],
		Stage::Diffuser => vec![param_slider(
			&params.diffuser_width,
			"Width",
			"Spreads each partial across neighbouring semitones.",
		)],
		Stage::VolumeNormalizer => vec![param_slider(
			&params.normalizer_rate,
			"Rate",
			"How fast the running peak follows the loudest partial.",
		)],
		Stage::Spiral => vec![
			pitch_range(params),
			key_row(&params.key),
			param_switch(
				&params.interval_ring,
				"Interval ring",
				&format!(
					"Names each spoke by its interval from the key. Takes {:.0} px from the spiral.",
					crate::graphic::generators::spiral::RING_ROOM
				),
			),
			param_slider(
				&params.exposure,
				"Exposure",
				"The gain on each note's light. Above 1× the notes near the loudest light fully and the loudest whitens.",
			),
			param_slider(
				&params.center_pad,
				"Centre hole",
				"The empty disc the lowest ring is drawn around.",
			),
			param_slider(
				&params.outer_pad,
				"Outer padding",
				"The margin between the highest ring and the nearer edge.",
			),
			param_slider(
				&params.glow_core,
				"Glow core",
				"The half-width of the ribbon's band at full brightness.",
			),
			param_slider(
				&params.glow_falloff,
				"Glow falloff",
				"The sigma of the glow past the core's edge.",
			),
		],
	};
	Column::with_children(groups).spacing(GROUP_SPACING).into()
}

/// A slider over `param`, under a row with its label and its value, over a dim
/// caption saying what it does.
///
/// The slider moves in the parameter's normalized range, and the parameter
/// rounds a value to its own steps, so one widget serves every kind.
pub fn param_slider<'a, P: Param>(
	param: &'a P,
	label: &'a str,
	caption_text: &'a str,
) -> Element<'a, Message> {
	let ptr = param.as_ptr();
	let step = param.step_count().map_or(0.001, |steps| 1.0 / steps as f32);
	let value = param.unmodulated_normalized_value();
	group(
		label_row(label, param.normalized_value_to_string(value, true)),
		slider(0.0..=1.0, value, move |normalized| {
			Message::Drag(ptr, normalized)
		})
		.on_release(Message::Release)
		.step(step)
		.into(),
		caption_text,
	)
}

/// The pitch range: the range in notes beside its label, the two-handle
/// slider, and the range's ends in hertz under it.
fn pitch_range(params: &PluginParams) -> Element<'_, Message> {
	let (lowest, highest) = params.pitch_range();
	let hz = |note: Note| caption(format!("{:.0} Hz", note.frequency()));
	column![
		label_row("Pitch range", format!("{lowest} – {highest}")),
		range_slider(&params.lowest_note, &params.highest_note),
		row![hz(lowest), space::horizontal(), hz(highest)],
	]
	.spacing(CONTROL_SPACING)
	.into()
}

/// The key row: twelve keys, one per pitch class, with the key the spiral is
/// set to down. The black keys are darker, as on a keyboard.
fn key_row(param: &IntParam) -> Element<'_, Message> {
	let down = param.value();
	let keys = KEYS.iter().enumerate().map(|(semitones, &(name, black))| {
		let semitones = semitones as i32;
		let is_down = semitones == down;
		let corners = border::Radius {
			top_left: if semitones == 0 { 6.0 } else { 0.0 },
			bottom_left: if semitones == 0 { 6.0 } else { 0.0 },
			top_right: if semitones == 11 { 6.0 } else { 0.0 },
			bottom_right: if semitones == 11 { 6.0 } else { 0.0 },
		};
		button(text(name).size(12).center().width(Length::Fill))
			.on_press(Message::Click(
				param.as_ptr(),
				param.preview_normalized(semitones),
			))
			.width(Length::Fill)
			.height(34)
			.padding(0)
			.style(move |theme: &Theme, status| {
				let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
				let background = if is_down {
					theme.palette().primary
				} else if hovered {
					Color::from_rgba(1.0, 1.0, 1.0, 0.14)
				} else if black {
					Color::from_rgba(0.0, 0.0, 0.0, 0.32)
				} else {
					WHITE_KEY
				};
				button::Style {
					background: Some(Background::Color(background)),
					text_color: if black && !is_down {
						Color::from_rgba(1.0, 1.0, 1.0, 0.7)
					} else {
						Color::WHITE
					},
					border: border::rounded(corners),
					..button::Style::default()
				}
			})
			.into()
	});
	column![
		text("Key").size(14),
		Row::with_children(keys).spacing(2),
		caption("Sits at the top of the wheel and anchors the hue.".to_string()),
	]
	.spacing(CONTROL_SPACING)
	.into()
}

/// A switch over `param`, at the end of its label's row, over a caption.
fn param_switch<'a>(
	param: &'a BoolParam,
	label: &'a str,
	caption_text: &str,
) -> Element<'a, Message> {
	let ptr = param.as_ptr();
	let switch = toggler(param.value())
		.on_toggle(move |on| Message::Click(ptr, if on { 1.0 } else { 0.0 }))
		.size(24)
		.style(switch_style);
	column![
		row![text(label).size(14), space::horizontal(), switch].align_y(Alignment::Center),
		caption(caption_text.to_string()),
	]
	.spacing(CONTROL_SPACING)
	.into()
}

/// A switch: a pill in the accent when on and a faint one when off, with a
/// white knob either way.
fn switch_style(theme: &Theme, status: toggler::Status) -> toggler::Style {
	let on = match status {
		toggler::Status::Active { is_toggled }
		| toggler::Status::Hovered { is_toggled }
		| toggler::Status::Disabled { is_toggled } => is_toggled,
	};
	toggler::Style {
		background: Background::Color(if on {
			theme.palette().primary
		} else {
			Color::from_rgba(1.0, 1.0, 1.0, 0.18)
		}),
		foreground: Background::Color(Color::WHITE),
		..toggler::default(theme, status)
	}
}

/// A control group: its label row, the control, and the caption under it.
pub fn group<'a>(
	label_row: Element<'a, Message>,
	control: Element<'a, Message>,
	caption_text: &'a str,
) -> Element<'a, Message> {
	column![label_row, control, caption(caption_text.to_string())]
		.spacing(CONTROL_SPACING)
		.into()
}

/// A control's label on the left and its current value on the right.
pub fn label_row<'a>(label: &'a str, value: String) -> Element<'a, Message> {
	row![
		text(label).size(14),
		space::horizontal(),
		text(value).size(14).color(INK_DIM)
	]
	.into()
}

pub fn caption<'a>(caption_text: String) -> Element<'a, Message> {
	text(caption_text).size(12).color(INK_DIM).into()
}

/// The pane's colours, as the theme the editor's widgets take theirs from: the
/// accent that fills sliders and switches, on the window's dark grey.
pub fn theme() -> Theme {
	Theme::custom(
		"Melody Visualizer",
		nice_plug_iced::iced::theme::Palette {
			background: Color::from_rgb8(0x24, 0x24, 0x24),
			text: Color::WHITE,
			primary: Color::from_rgb8(0x35, 0x84, 0xe4),
			success: Color::from_rgb8(0x57, 0xe3, 0x89),
			warning: Color::from_rgb8(0xf5, 0xc2, 0x11),
			danger: Color::from_rgb8(0xe0, 0x1b, 0x24),
		},
	)
}
