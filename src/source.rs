use jack::{Client, PortId, Port, Unowned};
use glib::Sender;

#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum SourceType {
	Audio,
	MIDI,
}

pub mod events {
	use jack::PortId;

	pub struct InputsChanged(pub PortId);
}

pub trait JackSource {
	fn client(&self) -> &Client;
	fn input_port(&self) -> &Port<Unowned>;
	fn source_type(&self) -> SourceType;
}