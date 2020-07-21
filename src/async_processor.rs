use futures::{prelude::*, channel::{mpsc, oneshot}};
use std::any::Any;

use crate::error::Error;

pub struct AsyncProcessor<T: ?Sized> {
	exec_tx: mpsc::Sender<Box<dyn FnOnce(&mut T) + Send>>,
}

impl<T: ?Sized> AsyncProcessor<T> {
	pub fn new(exec_tx: mpsc::Sender<Box<dyn FnOnce(&mut T) + Send>>) -> Self {
		AsyncProcessor {
			exec_tx,
		}
	}

	pub async fn exec<R, F>(&mut self, f: F) -> Result<R, Error>
		where
			R: Any + Send,
			F: FnOnce(&mut T) -> R + Send + 'static,
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
			F: FnOnce(&mut T) -> R + Send + 'static,
	{
		let mut self_clone = self.clone();
		async move { self_clone.exec(f).await }
	}

	pub async fn stop(&mut self) -> Result<(), Error> {
		if let Err(err) = self.exec_tx.close().await {
			if !err.is_disconnected() {
				return Err(Error::ProcessingControlError(err));
			}
		}
		Ok(())
	}
}

impl<T: ?Sized> Clone for AsyncProcessor<T> {
	fn clone(&self) -> Self {
		AsyncProcessor {
			exec_tx: self.exec_tx.clone(),
		}
	}
}
