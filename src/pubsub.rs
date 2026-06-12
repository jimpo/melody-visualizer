use async_channel::{Sender, TrySendError};
use std::{
	any::{Any, TypeId},
	cell::RefCell,
	collections::HashMap,
	rc::{Rc, Weak},
};

pub struct PubSub {
	subscribers: Rc<RefCell<HashMap<TypeId, Vec<Subscription>>>>,
	notifier: Notifier,
}

impl PubSub {
	pub fn new(context: Option<&glib::MainContext>, priority: glib::Priority) -> Self {
		let subscribers = Rc::new(RefCell::new(HashMap::new()));

		let (notification_tx, notification_rx) = async_channel::unbounded::<Box<dyn Any + Send>>();

		// Drain the channel on the GTK main loop. `Notifier` (Send) can be called
		// from any thread — notably JACK's notification thread — but this loop runs
		// on the main context, so subscriber callbacks always fire on the GTK
		// thread. (Replaces the deprecated `glib::MainContext::channel` + `attach`.)
		let subscribers_clone = subscribers.clone();
		let delivery_loop = async move {
			while let Ok(notification) = notification_rx.recv().await {
				notify(&mut subscribers_clone.borrow_mut(), notification);
			}
		};

		let main_context = context
			.cloned()
			.unwrap_or_else(glib::MainContext::ref_thread_default);
		// Dropping the returned JoinHandle detaches the task; it keeps running
		// until the channel closes (i.e. every `Notifier` has been dropped).
		main_context.spawn_local_with_priority(priority, delivery_loop);

		PubSub {
			subscribers,
			notifier: Notifier::new(notification_tx),
		}
	}

	pub fn notifier(&self) -> Notifier {
		self.notifier.clone()
	}

	pub fn subscribe<N, F>(&self, callback: F) -> SubscriptionHandle
	where
		N: Any + Send,
		F: Fn(&N) + 'static,
	{
		let callback: Rc<Box<dyn Fn(&(dyn Any + Send))>> = Rc::new(Box::new(move |notification| {
			let notification = notification.downcast_ref::<N>().expect(
				"all subscribers registered for a notification by TypeId will only be called \
					with notifications of the type matching that TypeId",
			);
			callback(notification);
		}));

		self.subscribers
			.borrow_mut()
			.entry(TypeId::of::<N>())
			.or_insert_with(Vec::new)
			.push(Subscription {
				callback: Rc::downgrade(&callback),
			});

		SubscriptionHandle(callback)
	}
}

fn notify(subscribers: &mut HashMap<TypeId, Vec<Subscription>>, notification: Box<dyn Any + Send>) {
	let type_id = (*notification).type_id();
	if let Some(subscribers) = subscribers.get_mut(&type_id) {
		let mut i = 0;
		while i < subscribers.len() {
			if subscribers[i].try_callback(&*notification) {
				i += 1;
			} else {
				subscribers.swap_remove(i);
			}
		}
	}
}

struct Subscription {
	callback: Weak<Box<dyn Fn(&(dyn Any + Send))>>,
}

impl Subscription {
	fn try_callback(&self, notification: &(dyn Any + Send)) -> bool {
		match self.callback.upgrade() {
			Some(callback) => {
				callback(notification);
				true
			}
			None => false,
		}
	}
}

#[derive(Clone)]
pub struct Notifier {
	notification_tx: Sender<Box<dyn Any + Send>>,
}

impl Notifier {
	fn new(notification_tx: Sender<Box<dyn Any + Send>>) -> Self {
		Notifier { notification_tx }
	}

	pub fn send<T: Any + Send>(
		&self,
		notification: T,
	) -> Result<(), TrySendError<Box<dyn Any + Send>>> {
		self.send_boxed(Box::new(notification))
	}

	pub fn send_boxed(
		&self,
		notification: Box<dyn Any + Send>,
	) -> Result<(), TrySendError<Box<dyn Any + Send>>> {
		// The channel is unbounded, so `try_send` only fails if it is closed and
		// never blocks — safe to call from the JACK notification thread.
		self.notification_tx.try_send(notification)
	}
}

#[derive(Clone)]
pub struct SubscriptionHandle(Rc<Box<dyn Fn(&(dyn Any + Send))>>);

#[cfg(test)]
mod tests {
	use super::*;
	use crate::test_support::run_in_glib_main_loop;
	use futures::prelude::*;
	use std::cell::RefCell;

	#[derive(Debug, PartialEq, Eq)]
	struct TestNotification;

	#[test]
	fn pubsub_subscriptions_are_called() {
		run_in_glib_main_loop(|mut yield_rx| async move {
			let pubsub = PubSub::new(None, glib::Priority::DEFAULT);
			let subscription_called = Rc::new(RefCell::new(false));

			let subscription_called_clone = subscription_called.clone();
			let handle = pubsub.subscribe(move |notification: &TestNotification| {
				assert_eq!(notification, &TestNotification);
				*subscription_called_clone.borrow_mut() = true;
			});

			pubsub
				.notifier()
				.send_boxed(Box::new(TestNotification))
				.unwrap();
			yield_rx.next().await.unwrap();

			assert!(*subscription_called.borrow());
		});
	}

	#[test]
	fn pubsub_dropped_subscriptions_are_not_called() {
		run_in_glib_main_loop(|mut yield_rx| async move {
			let pubsub = PubSub::new(None, glib::Priority::DEFAULT);
			let subscription_called = Rc::new(RefCell::new(false));

			let subscription_called_clone = subscription_called.clone();
			pubsub.subscribe(move |notification: &TestNotification| {
				*subscription_called_clone.borrow_mut() = true;
			});

			pubsub.notifier().send(TestNotification).unwrap();
			yield_rx.next().await.unwrap();

			assert!(!*subscription_called.borrow());
		});
	}

	#[test]
	fn pubsub_subscriptions_are_not_called_on_wrong_notification_type() {
		struct OtherTestNotification;

		run_in_glib_main_loop(|mut yield_rx| async move {
			let pubsub = PubSub::new(None, glib::Priority::DEFAULT);
			let subscription_called = Rc::new(RefCell::new(false));

			let subscription_called_clone = subscription_called.clone();
			pubsub.subscribe(move |notification: &TestNotification| {
				*subscription_called_clone.borrow_mut() = true;
			});

			pubsub
				.notifier()
				.send(Box::new(OtherTestNotification))
				.unwrap();
			yield_rx.next().await.unwrap();

			assert!(!*subscription_called.borrow());
		});
	}
}
