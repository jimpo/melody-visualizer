use std::error;

#[derive(Debug, derive_more::Display, derive_more::Error)]
pub enum Error {
	#[display(fmt = "glib error: {}", _0)]
	Glib(glib::Error),
	#[display(fmt = "JACK error: {}", _0)]
	Jack(jack::Error),
	#[display(fmt = "JACK non-empty client status: {:?}", _0)]
	JackStatus(#[error(not(source))] jack::ClientStatus),
	#[display(fmt = "failed to allocate a ring buffer of size {}", size)]
	RingBufferAllocFailure { size: usize },
}

impl From<jack::Error> for Error {
	fn from(err: jack::Error) -> Self {
		Error::Jack(err)
	}
}
