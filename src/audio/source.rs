#[derive(Debug, Clone, Copy, PartialEq, Eq, derive_more::Display)]
pub enum SourceType {
	Audio,
	MIDI,
}

/// The fully-qualified name of a JACK port, as `client:port`.
#[derive(
	Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, derive_more::Display, derive_more::From,
)]
pub struct PortName(pub String);

impl PortName {
	pub fn as_str(&self) -> &str {
		&self.0
	}
}

pub mod events {
	/// The set of ports JACK knows about changed — one was registered,
	/// unregistered, or renamed.
	///
	/// Which port changed is not carried, because no subscriber can use it:
	/// every one of them re-reads the whole list from
	/// [`JackSource::available_inputs`](super::JackSource::available_inputs),
	/// which is what keeps the list correct rather than incrementally patched.
	#[derive(Debug, Clone, PartialEq, Eq)]
	pub struct PortsChanged;

	#[derive(Debug, Clone, PartialEq, Eq)]
	pub struct SampleRateChanged(pub u32);
}

pub trait JackSource {
	fn client(&self) -> &jack::Client;
	fn input_port(&self) -> &jack::Port<jack::Unowned>;
	fn source_type(&self) -> SourceType;

	/// The ports that can be connected to this source's input, as JACK reports
	/// them right now.
	///
	/// Correct as of the [`PortsChanged`](events::PortsChanged) that prompted
	/// the call: the caller never has to wait for JACK to settle.
	fn available_inputs(&self) -> Vec<PortName>;
}
