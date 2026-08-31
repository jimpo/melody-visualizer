pub mod ports;
pub mod ring;
pub mod source;

use jack::{
	AudioIn, Client, ClientStatus, Control, Frames, NotificationHandler, Port, PortId,
	ProcessHandler, ProcessScope,
};
use log::error;

use crate::error::Error;
use crate::pubsub::Notifier;
use ports::RetiredPorts;
use ring::{SampleWriter, sample_ring};
use source::{JackSource, PortName, SourceType, events};

pub use ring::SampleReader;

const TITLE: &str = "Melody Visualizer";

pub struct AudioSourceController {
	client: jack::AsyncClient<AudioNotificationHandler, AudioProcessHandler>,
	input_port: jack::Port<jack::Unowned>,
	retired_ports: RetiredPorts,
}

impl AudioSourceController {
	pub fn new(buffer_size: usize, notifier: Notifier) -> Result<(Self, SampleReader), Error> {
		let (client, status) =
			jack::Client::new(TITLE, jack::ClientOptions::NO_START_SERVER).map_err(Error::Jack)?;
		if !status.is_empty() {
			return Err(Error::JackStatus(status));
		}

		let port = client
			.register_port("input", AudioIn::default())
			.map_err(Error::Jack)?;
		let input_port = port.clone_unowned();

		let (writer, reader) = sample_ring(buffer_size)?;
		let retired_ports = RetiredPorts::default();
		let client = client
			.activate_async(
				AudioNotificationHandler::new(notifier, retired_ports.clone()),
				AudioProcessHandler::new(port, writer),
			)
			.map_err(Error::Jack)?;
		let controller = AudioSourceController {
			client,
			input_port,
			retired_ports,
		};
		Ok((controller, reader))
	}
}

struct AudioNotificationHandler {
	notifier: Notifier,
	retired_ports: RetiredPorts,
}

impl AudioNotificationHandler {
	fn new(notifier: Notifier, retired_ports: RetiredPorts) -> Self {
		AudioNotificationHandler {
			notifier,
			retired_ports,
		}
	}

	// The methods below hold the event-forwarding logic, free of any JACK runtime
	// types (`&Client`), so they can be unit-tested without a live JACK server.
	// The `NotificationHandler` trait impl is a thin wrapper around them.

	fn notify_sample_rate(&self, sample_rate: Frames) {
		if let Err(err) = self.notifier.send(events::SampleRateChanged(sample_rate)) {
			error!("failed to notify of JACK sample rate change: {}", err);
		}
	}

	/// Record what a port registration means for the port list, then announce
	/// that the list changed.
	///
	/// The name is passed in because resolving it needs the `&Client` only the
	/// JACK callback holds. An unregistered port stays listed by JACK for a
	/// short while afterwards, so its name is retired until JACK catches up;
	/// see [`ports::RetiredPorts`].
	fn notify_port_registration(&self, port_name: Option<String>, is_registered: bool) {
		match port_name {
			Some(name) if is_registered => self.retired_ports.restore(&name),
			Some(name) => self.retired_ports.retire(name),
			// Nothing to retire, so a removed port may linger in the list until
			// the next port change refreshes it.
			None => error!("a JACK port changed registration without a name"),
		}
		self.notify_ports_changed();
	}

	fn notify_ports_changed(&self) {
		if let Err(err) = self.notifier.send(events::PortsChanged) {
			error!("failed to notify of JACK port change: {}", err);
		}
	}
}

impl NotificationHandler for AudioNotificationHandler {
	// TODO: Handle shutdown gracefully
	unsafe fn shutdown(&mut self, status: ClientStatus, reason: &str) {
		error!(
			"JACK client shutdown: status = {:?}, reason = {}",
			status, reason
		);
	}

	fn sample_rate(&mut self, _client: &Client, sample_rate: Frames) -> Control {
		self.notify_sample_rate(sample_rate);
		Control::Continue
	}

	fn port_registration(&mut self, client: &Client, port_id: PortId, is_registered: bool) {
		self.notify_port_registration(port_name(client, port_id), is_registered);
	}

	fn port_rename(
		&mut self,
		_: &Client,
		_port_id: PortId,
		_old_name: &str,
		_new_name: &str,
	) -> Control {
		self.notify_ports_changed();
		Control::Continue
	}
}

/// The fully-qualified name of the port JACK identified by `port_id`.
fn port_name(client: &Client, port_id: PortId) -> Option<String> {
	let port = client.port_by_id(port_id)?;
	port.name()
		.inspect_err(|err| error!("JACK port {} has no name: {}", port_id, err))
		.ok()
}

struct AudioProcessHandler {
	port: Port<AudioIn>,
	writer: SampleWriter,
}

impl AudioProcessHandler {
	fn new(port: Port<AudioIn>, writer: SampleWriter) -> Self {
		AudioProcessHandler { port, writer }
	}
}

impl ProcessHandler for AudioProcessHandler {
	fn process(&mut self, _client: &Client, scope: &ProcessScope) -> Control {
		self.writer.write_samples(self.port.as_slice(scope));
		Control::Continue
	}
}

impl JackSource for AudioSourceController {
	fn client(&self) -> &jack::Client {
		self.client.as_client()
	}

	fn input_port(&self) -> &jack::Port<jack::Unowned> {
		&self.input_port
	}

	fn source_type(&self) -> SourceType {
		SourceType::Audio
	}

	fn available_inputs(&self) -> Vec<PortName> {
		ports::audio_outputs(self.client.as_client(), &self.retired_ports)
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::pubsub::PubSub;
	use crate::test_support::run_in_glib_main_loop;
	use futures::prelude::*;
	use source::events::{PortsChanged, SampleRateChanged};
	use std::cell::RefCell;
	use std::rc::Rc;

	// --- JACK notification forwarding -------------------------------------
	//
	// These drive `AudioNotificationHandler`'s client-free logic methods and
	// assert the events reach a `PubSub` subscriber. No JACK server required.

	#[test]
	fn notification_handler_forwards_sample_rate() {
		run_in_glib_main_loop(|mut yield_rx| async move {
			let pubsub = PubSub::new(None, glib::Priority::DEFAULT);
			let received = Rc::new(RefCell::new(None));

			let received_clone = received.clone();
			let _handle = pubsub.subscribe(move |event: &SampleRateChanged| {
				*received_clone.borrow_mut() = Some(event.clone());
			});

			let handler = AudioNotificationHandler::new(pubsub.notifier(), RetiredPorts::default());
			handler.notify_sample_rate(48_000);
			yield_rx.next().await.unwrap();

			assert_eq!(*received.borrow(), Some(SampleRateChanged(48_000)));
		});
	}

	#[test]
	fn notification_handler_forwards_every_port_change() {
		run_in_glib_main_loop(|mut yield_rx| async move {
			let pubsub = PubSub::new(None, glib::Priority::DEFAULT);
			let received = Rc::new(RefCell::new(Vec::new()));

			let received_clone = received.clone();
			let _handle = pubsub.subscribe(move |event: &PortsChanged| {
				received_clone.borrow_mut().push(event.clone());
			});

			let handler = AudioNotificationHandler::new(pubsub.notifier(), RetiredPorts::default());
			handler.notify_port_registration(Some("tone:output1".to_string()), true);
			handler.notify_port_registration(Some("tone:output1".to_string()), false);
			handler.notify_ports_changed();
			yield_rx.next().await.unwrap();

			assert_eq!(
				*received.borrow(),
				vec![PortsChanged, PortsChanged, PortsChanged],
				"a registration, an unregistration and a rename each announce the list",
			);
		});
	}

	#[test]
	fn notification_handler_retires_an_unregistered_port() {
		let pubsub = PubSub::new(None, glib::Priority::DEFAULT);
		let retired_ports = RetiredPorts::default();
		let handler = AudioNotificationHandler::new(pubsub.notifier(), retired_ports.clone());

		handler.notify_port_registration(Some("tone:output1".to_string()), false);

		assert_eq!(
			retired_ports.subtract(vec!["tone:output1".to_string()]),
			[],
			"an unregistered port is hidden from the moment JACK announces it",
		);
	}

	// --- Integration: real JACK server ------------------------------------
	//
	// Exercises the parts the unit tests above cannot: real client creation,
	// input-port registration, and `activate_async`. Ignored by default so plain
	// `cargo test` stays deterministic with no server. Run against a dummy server:
	//
	//   jackd -r -d dummy &            # or `scripts/run-headless.sh`'s setup
	//   cargo test -- --ignored

	#[test]
	#[ignore = "requires a running JACK server (e.g. `jackd -d dummy`)"]
	fn audio_source_controller_connects_to_running_jack_server() {
		let pubsub = PubSub::new(None, glib::Priority::DEFAULT);

		let (controller, _reader) = AudioSourceController::new(128 * 1024, pubsub.notifier())
			.expect("should connect to the running JACK server and register its input port");

		assert_eq!(controller.source_type(), SourceType::Audio);
		// The input port was really registered with the server, so it has a name.
		assert!(
			controller.input_port().name().is_ok(),
			"registered input port should have a name",
		);
		// Every JACK server has capture ports, and they are what the app connects
		// to its input. Its own input port is not among them: it is an input.
		let inputs = controller.available_inputs();
		assert!(
			inputs
				.iter()
				.any(|port| port.as_str().starts_with("system:capture_")),
			"the server's capture ports should be offered as inputs, got {:?}",
			inputs,
		);
		assert!(
			!inputs.contains(&PortName(controller.input_port().name().unwrap())),
			"the source's own input port is not something it can connect to",
		);
	}
}
