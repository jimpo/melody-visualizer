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

/// A unit of work shipped to the thread that owns the `T`.
pub type ExecCommand<T> = Box<dyn FnOnce(&mut T) + Send>;
pub type ExecSender<T> = mpsc::Sender<ExecCommand<T>>;
pub type ExecReceiver<T> = mpsc::Receiver<ExecCommand<T>>;

pub struct AsyncProcessor<T: ?Sized> {
	exec_tx: ExecSender<T>,
}

impl<T: ?Sized> AsyncProcessor<T> {
	pub fn new(exec_tx: ExecSender<T>) -> Self {
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

	/// Closes the command channel, which ends the processor's loop.
	///
	/// The channel is closed for every clone of this `AsyncProcessor`, not just
	/// for this one, so the processor stops even while other handles to it are
	/// alive. Commands already queued are still delivered; later ones fail with
	/// [`CommunicationError::DeliveryFailure`].
	pub fn stop(&mut self) {
		self.exec_tx.close_channel();
	}
}

impl<T: ?Sized> Clone for AsyncProcessor<T> {
	fn clone(&self) -> Self {
		AsyncProcessor {
			exec_tx: self.exec_tx.clone(),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures::executor::block_on;

	#[test]
	fn stopping_one_handle_stops_the_processor_for_all_of_them() {
		let (exec_tx, mut exec_rx) = mpsc::channel::<ExecCommand<u32>>(0);
		let processor = AsyncProcessor::new(exec_tx);

		let mut handle = processor.clone();
		handle.stop();

		assert!(block_on(exec_rx.next()).is_none());
		assert!(block_on(processor.exec_cloned(|value: &mut u32| *value)).is_err());
	}
}
