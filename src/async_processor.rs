use futures::{
	channel::{mpsc, oneshot},
	prelude::*,
};
use std::any::Any;

#[derive(Debug, derive_more::Display, derive_more::From, derive_more::Error)]
pub enum CommunicationError {
	#[display("failed to send message to processor: {}", _0)]
	DeliveryFailure(mpsc::SendError),
	#[display("processor failed to send response")]
	ResponseFailure,
}

pub struct AsyncProcessor<T: ?Sized> {
	exec_tx: mpsc::Sender<Box<dyn FnOnce(&mut T) + Send>>,
}

impl<T: ?Sized> AsyncProcessor<T> {
	pub fn new(exec_tx: mpsc::Sender<Box<dyn FnOnce(&mut T) + Send>>) -> Self {
		AsyncProcessor { exec_tx }
	}

	pub async fn exec<R, F>(&mut self, f: F) -> Result<R, CommunicationError>
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
			.await?;

		response_rx
			.await
			.map_err(|_| CommunicationError::ResponseFailure)
	}

	pub fn exec_cloned<R, F>(
		&self,
		f: F,
	) -> impl Future<Output = Result<R, CommunicationError>> + use<R, F, T>
	where
		R: Any + Send,
		F: FnOnce(&mut T) -> R + Send + 'static,
	{
		let mut self_clone = self.clone();
		async move { self_clone.exec(f).await }
	}

	pub async fn stop(&mut self) -> Result<(), CommunicationError> {
		if let Err(err) = self.exec_tx.close().await {
			if !err.is_disconnected() {
				return Err(err.into());
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
