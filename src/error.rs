use std::error;
use std::fmt;

#[derive(Debug)]
pub enum Error {
	Glib(glib::Error),
	Jack(jack::Error),
	JackStatus(jack::ClientStatus),
}

impl fmt::Display for Error {
	fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
		match self {
			Error::Glib(err) => write!(f, "glib error: {}", err),
			Error::Jack(err) => write!(f, "JACK error: {}", err),
			Error::JackStatus(status) => write!(f, "JACK non-empty client status: {:?}", status),
		}
	}
}

impl error::Error for Error {
	fn source(&self) -> Option<&(dyn error::Error + 'static)> {
		match self {
			Error::Glib(err) => Some(err),
			Error::Jack(err) => Some(err),
			Error::JackStatus(_) => None,
		}
	}
}