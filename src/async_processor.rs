use futures::{prelude::*, channel::{mpsc, oneshot}};
use std::any::Any;

use crate::error::Error;

pub struct AsyncProcessor<Cmd: Send, T: ?Sized> {
	control_tx: mpsc::Sender<(Cmd, oneshot::Sender<Box<dyn Any + Send>>)>,
	exec_tx: mpsc::Sender<Box<dyn FnOnce(&mut T) + Send>>,
}

impl<Cmd: Send, T: ?Sized> AsyncProcessor<Cmd, T> {
	pub fn new(
		control_tx: mpsc::Sender<(Cmd, oneshot::Sender<Box<dyn Any + Send>>)>,
		exec_tx: mpsc::Sender<Box<dyn FnOnce(&mut T) + Send>>,
	) -> Self {
		AsyncProcessor {
			control_tx,
			exec_tx,
		}
	}

	pub async fn exec<R, F>(&mut self, f: F) -> Result<R, Error>
		where
			R: Any + Send,
			F: Fn(&mut T) -> R + Send + 'static,
	{
		let (response_tx, response_rx) = oneshot::channel();
		self.exec_tx
			.send(Box::new(move |arg| {
				let result = f(arg);
				// Error indicates that the exec caller dropped the returned Future. Ignore this.
				let _ = response_tx.send(result);
			}))
			.await
			.map_err(Error::ProcessingControlError)?;

		response_rx.await
			.map_err(|_| Error::AsyncCallFailure)
	}

	pub fn exec_cloned<R, F>(&self, f: F) -> impl Future<Output=Result<R, Error>>
		where
			R: Any + Send,
			F: Fn(&mut T) -> R + Send + 'static,
	{
		let mut self_clone = self.clone();
		async move { self_clone.exec(f).await }
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

	pub fn call_cloned<R: Any + Send>(&self, cmd: Cmd) -> impl Future<Output=Result<R, Error>> {
		let mut self_clone = self.clone();
		async move { self_clone.call(cmd).await }
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

impl<Cmd: Send, T: ?Sized> Clone for AsyncProcessor<Cmd, T> {
	fn clone(&self) -> Self {
		AsyncProcessor {
			control_tx: self.control_tx.clone(),
			exec_tx: self.exec_tx.clone(),
		}
	}
}
