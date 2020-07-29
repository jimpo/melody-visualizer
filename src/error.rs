use std::any::Any;
use std::io;
use std::sync::mpsc::SendError;

use crate::async_processor::CommunicationError;

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
	#[display(fmt = "communication error with background processor: {}", _0)]
	Communication(CommunicationError),
	#[display(fmt = "the JACK client is not active")]
	NoJackSource,
	#[display(fmt = "failed to publish notification: {:?}", _0)]
	#[from(ignore)]
	PubSub(SendError<Box<dyn Any + Send>>),
}
