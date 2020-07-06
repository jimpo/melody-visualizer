use jack::{AudioIn, NotificationHandler, RingBufferWriter, ProcessHandler, ProcessScope, Control, RingBuffer, Client, Port, PortId, ClientStatus, RingBufferReader};
use log::error;
use std::sync::{Arc, Mutex};

use crate::error::Error;
use crate::source::{JackSource, SourceSignals, SourceType};

const TITLE: &str = "Melody Visualizer";

pub struct AudioSourceController {
	client: jack::AsyncClient<AudioNotificationHandler, AudioProcessHandler>,
	input_port: jack::Port<jack::Unowned>,
}

impl AudioSourceController {
	pub fn new(buffer_size: usize, signals: Arc<SourceSignals>) -> Result<(Self, RingBufferReader), Error> {
		let (client, status) = jack::Client::new(TITLE, jack::ClientOptions::NO_START_SERVER)
			.map_err(Error::Jack)?;
		if !status.is_empty() {
			return Err(Error::JackStatus(status));
		}

		let port = client.register_port("input", AudioIn)
			.map_err(Error::Jack)?;
		let input_port = port.clone_unowned();

		let (buffer_reader, buffer_writer) = RingBuffer::new(buffer_size)
			.map_err(|()| Error::RingBufferAllocFailure { size: buffer_size })?
			.into_reader_writer();
		let client = client.activate_async(
			AudioNotificationHandler::new(signals),
			AudioProcessHandler::new(port, buffer_writer),
		)
			.map_err(Error::Jack)?;
		Ok(AudioSourceController {
			client,
			input_port,
		})
	}
}

struct AudioNotificationHandler {
	signals: Arc<SourceSignals>,
}

impl AudioNotificationHandler {
	fn new(signals: Arc<SourceSignals>) -> Self {
		AudioNotificationHandler {
			signals,
		}
	}
}

impl NotificationHandler for AudioNotificationHandler {
	// TODO: Handle shutdown gracefully
	fn shutdown(&mut self, status: ClientStatus, reason: &str) {
		error!("JACK client shutdown: status = {:?}, reason = {}", status, reason);
	}

	fn port_registration(&mut self, _client: &Client, port_id: PortId, _is_registered: bool) {
		if let Err(err) = self.signals.on_inputs_changed.send(port_id) {
			error!("failed to signal JACK input change to main context: {}", err);
		}
	}

	fn port_rename(&mut self, _: &Client, port_id: PortId, _old_name: &str, _new_name: &str)
		-> Control
	{
		if let Err(err) = self.signals.on_inputs_changed.send(port_id) {
			error!("failed to signal JACK input change to main context: {}", err);
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
	fn process(&mut self, client: &Client, scope: &ProcessScope) -> Control {
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
