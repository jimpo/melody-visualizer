use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::sync::mpsc::SendError;

pub struct PubSub {
	subscribers: Rc<RefCell<HashMap<TypeId, Vec<Subscription>>>>,
	notifier: Notifier,
}

impl PubSub {
	pub fn new(context: Option<&glib::MainContext>, priority: glib::Priority) -> Self {
		let subscribers = Rc::new(RefCell::new(HashMap::new()));

		let (notification_tx, notification_rx) = glib::MainContext::channel(priority);

		let subscribers_clone = subscribers.clone();
		notification_rx.attach(context, move |notification| {
			notify(&mut *subscribers_clone.borrow_mut(), notification);
			glib::Continue(true)
		});

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
			F: Fn(&N) + 'static
	{
		let callback: Rc<Box<dyn Fn(&(dyn Any + Send))>> = Rc::new(Box::new(move |notification| {
			let notification = notification.downcast_ref::<N>()
				.expect(
					"all subscribers registered for a notification by TypeId will only be called \
					with notifications of the type matching that TypeId"
				);
			callback(notification);
		}));

		self.subscribers
			.borrow_mut()
			.entry(TypeId::of::<N>())
			.or_insert_with(Vec::new)
			.push(Subscription { callback: Rc::downgrade(&callback) });

		SubscriptionHandle(callback)
	}
}

fn notify(
	subscribers: &mut HashMap<TypeId, Vec<Subscription>>,
	notification: Box<dyn Any + Send>
) {
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
	notification_tx: glib::Sender<Box<dyn Any + Send>>,
}

impl Notifier {
	fn new(notification_tx: glib::Sender<Box<dyn Any + Send>>) -> Self {
		Notifier { notification_tx }
	}

	pub fn send<T: Any + Send>(&self, notification: T)
		-> Result<(), SendError<Box<dyn Any + Send>>>
	{
		self.send_boxed(Box::new(notification))
	}

	pub fn send_boxed(&self, notification: Box<dyn Any + Send>)
		-> Result<(), SendError<Box<dyn Any + Send>>>
	{
		self.notification_tx.send(notification)
	}
}

#[derive(Clone)]
pub struct SubscriptionHandle(Rc<Box<dyn Fn(&(dyn Any + Send))>>);

#[cfg(test)]
mod tests {
	use super::*;
	use futures::{prelude::*, channel::mpsc};
	use glib::MainLoop;
	use std::cell::RefCell;
	use std::sync::Arc;
	use std::thread_local;

	fn run_in_glib_main_loop<F, U>(f: F)
		where F: FnOnce(mpsc::Receiver<()>) -> U + Send + 'static,
			  U: Future<Output = ()>,
	{
		let main_loop = Arc::new(MainLoop::new(None, false));
		let main_context = main_loop.get_context();

		// This seems kind of wacky. The idea is that we'll have an asynchronous test case that can
		// yield control back to the main loop. The way we do that is have a separate low-priority
		// task, and every time it is polled, it will wake up the test code. In effect, every time
		// the test code yields, it will be resumed when no tasks higher than PRIORITY_LOW are
		// ready.
		let (mut yield_tx, yield_rx) = mpsc::channel(0);

		main_context.spawn_with_priority(glib::PRIORITY_LOW, async move {
			loop {
				yield_tx.send(()).await.unwrap();
			}
		});

		let main_loop_clone = main_loop.clone();
		main_context.spawn(async move {
			let main_context = main_loop_clone.get_context();
			main_context.spawn_local(async move {
				// Quit main loop after test code completes.
				f(yield_rx).await;
				main_loop_clone.quit();
			});
		});

		main_loop.run();
	}

	#[derive(Debug, PartialEq, Eq)]
	struct TestNotification;

	#[test]
	fn pubsub_subscriptions_are_called() {
		run_in_glib_main_loop(|mut yield_rx| async move {
			let pubsub = PubSub::new(None, glib::PRIORITY_DEFAULT);
			let subscription_called = Rc::new(RefCell::new(false));

			let subscription_called_clone = subscription_called.clone();
			let handle = pubsub.subscribe(move |notification: &TestNotification| {
				assert_eq!(notification, &TestNotification);
				*subscription_called_clone.borrow_mut() = true;
			});

			pubsub.notifier().send_boxed(Box::new(TestNotification)).unwrap();
			yield_rx.next().await.unwrap();

			assert!(*subscription_called.borrow());
		});
	}

	#[test]
	fn pubsub_dropped_subscriptions_are_not_called() {
		run_in_glib_main_loop(|mut yield_rx| async move {
			let pubsub = PubSub::new(None, glib::PRIORITY_DEFAULT);
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
			let pubsub = PubSub::new(None, glib::PRIORITY_DEFAULT);
			let subscription_called = Rc::new(RefCell::new(false));

			let subscription_called_clone = subscription_called.clone();
			pubsub.subscribe(move |notification: &TestNotification| {
				*subscription_called_clone.borrow_mut() = true;
			});

			pubsub.notifier().send(Box::new(OtherTestNotification)).unwrap();
			yield_rx.next().await.unwrap();

			assert!(!*subscription_called.borrow());
		});
	}
}
