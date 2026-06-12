use jack::{
	AudioIn, Client, ClientStatus, Control, Frames, NotificationHandler, Port, PortId,
	ProcessHandler, ProcessScope, RingBuffer, RingBufferWriter, RingBufferReader,
};
use log::error;
use std::sync::Mutex;

use crate::error::Error;
use crate::pubsub::Notifier;
use crate::source::{events, JackSource, SourceType};

const TITLE: &str = "Melody Visualizer";

pub struct AudioSourceController {
	client: jack::AsyncClient<AudioNotificationHandler, AudioProcessHandler>,
	input_port: jack::Port<jack::Unowned>,
}

impl AudioSourceController {
	pub fn new(buffer_size: usize, notifier: Notifier) -> Result<(Self, RingBufferReader), Error> {
		let (client, status) = jack::Client::new(TITLE, jack::ClientOptions::NO_START_SERVER)
			.map_err(Error::Jack)?;
		if !status.is_empty() {
			return Err(Error::JackStatus(status));
		}

		let port = client.register_port("input", AudioIn::default())
			.map_err(Error::Jack)?;
		let input_port = port.clone_unowned();

		let (buffer_reader, buffer_writer) = RingBuffer::new(buffer_size)
			.map_err(|_| Error::RingBufferAllocFailure { size: buffer_size })?
			.into_reader_writer();
		let client = client.activate_async(
			AudioNotificationHandler::new(notifier),
			AudioProcessHandler::new(port, buffer_writer),
		)
			.map_err(Error::Jack)?;
		let controller = AudioSourceController {
			client,
			input_port,
		};
		Ok((controller, buffer_reader))
	}
}

struct AudioNotificationHandler {
	notifier: Notifier,
}

impl AudioNotificationHandler {
	fn new(notifier: Notifier) -> Self {
		AudioNotificationHandler { notifier }
	}
}

impl NotificationHandler for AudioNotificationHandler {
	// TODO: Handle shutdown gracefully
	unsafe fn shutdown(&mut self, status: ClientStatus, reason: &str) {
		error!("JACK client shutdown: status = {:?}, reason = {}", status, reason);
	}

	fn sample_rate(&mut self, _client: &Client, sample_rate: Frames) -> Control {
		if let Err(err) = self.notifier.send(events::SampleRateChanged(sample_rate)) {
			error!("failed to notify of JACK sample rate change: {}", err);
		}
		Control::Continue
	}

	fn port_registration(&mut self, _client: &Client, port_id: PortId, is_registered: bool) {
		let notification = if is_registered {
			events::InputsChanged::Registered(port_id)
		} else {
			events::InputsChanged::Unregistered(port_id)
		};
		if let Err(err) = self.notifier.send(notification) {
			error!("failed to notify of JACK input change: {}", err);
		}
	}

	fn port_rename(&mut self, _: &Client, port_id: PortId, _old_name: &str, new_name: &str)
		-> Control
	{
		let notification = events::InputsChanged::Renamed(port_id, new_name.into());
		if let Err(err) = self.notifier.send(notification) {
			error!("failed to notify of JACK input change: {}", err);
		}
		Control::Continue
	}
}

struct AudioProcessHandler {
	port: Port<AudioIn>,
	// This should not need a Mutex, but it does.
	// https://github.com/RustAudio/rust-jack/issues/121
	ring_buffer: Mutex<RingBufferWriter>,
}

impl AudioProcessHandler {
	fn new(port: Port<AudioIn>, ring_buffer: RingBufferWriter) -> Self {
		AudioProcessHandler {
			port,
			ring_buffer: Mutex::new(ring_buffer),
		}
	}
}

impl ProcessHandler for AudioProcessHandler {
	fn process(&mut self, _client: &Client, scope: &ProcessScope) -> Control {
		let mut ring_buffer = self.ring_buffer.lock()
			.expect("I shouldn't even need a Mutex...");
		// TODO: Create a custom ring buffer holding an [f32] that is more efficient.
		for sample in self.port.as_slice(scope) {
			ring_buffer.write_buffer(&sample.to_ne_bytes());
		}
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
}
