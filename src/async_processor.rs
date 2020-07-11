use futures::{prelude::*, channel::{mpsc, oneshot}};
use std::any::Any;

use crate::error::Error;

pub struct AsyncProcessor<Cmd: Send> {
	control_tx: mpsc::Sender<(Cmd, oneshot::Sender<Box<dyn Any + Send>>)>,
}

impl<Cmd: Send> AsyncProcessor<Cmd> {
	pub fn new(control_tx: mpsc::Sender<(Cmd, oneshot::Sender<Box<dyn Any + Send>>)>) -> Self {
		AsyncProcessor {
			control_tx,
		}
	}

	pub async fn call<R: Any + Send>(&mut self, cmd: Cmd) -> Result<R, Error> {
		let (reply_tx, reply_rx) = oneshot::channel();
		self.control_tx.send((cmd, reply_tx)).await
			.map_err(Error::ProcessingControlError)?;

		let reply_untyped = reply_rx.await
			.map_err(|_| Error::AsyncCallFailure)?;
		let reply = reply_untyped.downcast()
			.map_err(|_| Error::AsyncCallFailure)?;
		Ok(*reply)
	}

	pub async fn stop(&mut self) -> Result<(), Error> {
		if let Err(err) = self.control_tx.close().await {
			if !err.is_disconnected() {
				return Err(Error::ProcessingControlError(err));
			}
		}
		Ok(())
	}
}

impl<Cmd: Send> Clone for AsyncProcessor<Cmd> {
	fn clone(&self) -> Self {
		AsyncProcessor {
			control_tx: self.control_tx.clone(),
		}
	}
}
