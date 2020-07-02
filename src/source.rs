use jack::{Client, PortId};
use glib::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum SourceType {
	Audio,
	MIDI,
}

pub struct SourceSignals {
	pub on_inputs_changed: Sender<PortId>,
}

pub trait JackSource {
	fn client(&self) -> &Client;
	fn source_type(&self) -> SourceType;
}