use crate::error::Error;

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

	/// Something was connected to or disconnected from the source's input port.
	///
	/// Fired for connections the app made and for ones made behind its back,
	/// because both reach it the same way — through JACK's callback.
	#[derive(Debug, Clone, PartialEq, Eq)]
	pub struct ConnectionChanged;

	#[derive(Debug, Clone, PartialEq, Eq)]
	pub struct SampleRateChanged(pub u32);
}

pub trait JackSource {
	fn source_type(&self) -> SourceType;

	/// The rate the audio server is running at, in frames per second.
	fn sample_rate(&self) -> u32;

	/// The ports that can be connected to this source's input, as JACK reports
	/// them right now.
	///
	/// Correct as of the [`PortsChanged`](events::PortsChanged) that prompted
	/// the call: the caller never has to wait for JACK to settle.
	fn available_inputs(&self) -> Vec<PortName>;

	/// The port feeding this source's input, or `None` if nothing is.
	///
	/// Read from the server on every call, so it tells the truth about
	/// connections made outside the app as well as inside it.
	fn connected_input(&self) -> Option<PortName>;

	/// Feed this source's input from `port`, in place of whatever fed it before.
	fn connect(&self, port: &PortName) -> Result<(), Error>;

	/// Stop feeding this source's input from anything.
	fn disconnect(&self) -> Result<(), Error>;
}
