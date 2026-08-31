use async_channel::TrySendError;
use std::any::Any;
use std::io;

use crate::async_processor::CommunicationError;
use crate::spectrum::TransformId;

#[derive(Debug, derive_more::Display, derive_more::From, derive_more::Error)]
pub enum Error {
	#[display("Cairo error: {}", _0)]
	Cairo(cairo::Error),
	#[display("glib error: {}", _0)]
	Glib(glib::Error),
	#[display("JACK error: {}", _0)]
	Jack(jack::Error),
	#[display("JACK non-empty client status: {:?}", _0)]
	JackStatus(#[error(not(source))] jack::ClientStatus),
	#[display("failed to allocate a ring buffer of size {}", size)]
	#[from(skip)]
	RingBufferAllocFailure {
		size: usize,
	},
	#[display("failed to spawn a new thread: {}", _0)]
	ThreadSpawnFailure(io::Error),
	GraphicDrawClonesSurface,
	#[display("communication error with background processor: {}", _0)]
	Communication(CommunicationError),
	#[display("the JACK client is not active")]
	NoJackSource,
	#[display("failed to publish notification: {:?}", _0)]
	#[from(skip)]
	PubSub(TrySendError<Box<dyn Any + Send>>),
	#[display("config references missing transform with ID {}", id)]
	#[from(skip)]
	MissingTransform {
		id: TransformId,
	},
	#[display("invalid config entry reference: {}", _0)]
	UnexpectedConfigEntry(#[error(not(source))] String),
}
