use jack::{Client, Port, Unowned};

#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum SourceType {
	Audio,
	MIDI,
}

pub mod events {
	use jack::{Frames, PortId};

	#[derive(Debug, Clone)]
	pub enum InputsChanged {
		Registered(PortId),
		Unregistered(PortId),
		Renamed(PortId, String),
	}

	#[derive(Debug, Clone)]
	pub struct SampleRateChanged(pub Frames);
}

pub trait JackSource {
	fn client(&self) -> &Client;
	fn input_port(&self) -> &Port<Unowned>;
	fn source_type(&self) -> SourceType;
}