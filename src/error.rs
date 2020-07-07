use futures::channel::mpsc;
use std::io;

#[derive(Debug, derive_more::Display, derive_more::From, derive_more::Error)]
pub enum Error {
	#[display(fmt = "Cairo error: {}", _0)]
	Cairo(cairo::Error),
	#[display(fmt = "glib error: {}", _0)]
	Glib(glib::Error),
	#[display(fmt = "JACK error: {}", _0)]
	Jack(jack::Error),
	#[display(fmt = "JACK non-empty client status: {:?}", _0)]
	JackStatus(#[error(not(source))] jack::ClientStatus),
	#[display(fmt = "failed to allocate a ring buffer of size {}", size)]
	#[from(ignore)]
	RingBufferAllocFailure { size: usize },
	#[display(fmt = "failed to spawn a new thread: {}", _0)]
	ThreadSpawnFailure(io::Error),
	GraphicDrawClonesSurface,
	#[display(fmt = "failed to send control command to processing thread: {}", _0)]
	#[from(ignore)]
	ProcessingControlError(mpsc::SendError),
}
