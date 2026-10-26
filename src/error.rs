use std::io;

use crate::async_processor::CommunicationError;
use crate::spectrum::TransformId;

#[derive(Debug, derive_more::Display, derive_more::From, derive_more::Error)]
pub enum Error {
	#[display("failed to spawn a new thread: {}", _0)]
	ThreadSpawnFailure(io::Error),
	#[display("communication error with background processor: {}", _0)]
	Communication(CommunicationError),
	#[display("config references missing transform with ID {}", id)]
	#[from(skip)]
	MissingTransform { id: TransformId },
	#[display("invalid config entry reference: {}", _0)]
	UnexpectedConfigEntry(#[error(not(source))] String),
}
