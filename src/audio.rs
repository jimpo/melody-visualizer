use jack::{
	AudioIn, NotificationHandler, RingBufferWriter, ProcessHandler, ProcessScope, Control, RingBuffer, Client, PortId, ClientStatus,
};
use log::error;
use std::sync::{Arc, Mutex};

use crate::error::Error;
use crate::source::{JackSource, SourceSignals, SourceType};

const TITLE: &str = "Melody Visualizer";

pub struct AudioSourceController {
	client: jack::AsyncClient<AudioNotificationHandler, AudioProcessHandler>,
}

impl AudioSourceController {
	pub fn new(buffer_size: usize, signals: Arc<SourceSignals>) -> Result<Self, Error> {
		let (client, status) = jack::Client::new(TITLE, jack::ClientOptions::NO_START_SERVER)
			.map_err(Error::Jack)?;
		if !status.is_empty() {
			return Err(Error::JackStatus(status));
		}

		let port = client.register_port("input", AudioIn)
			.map_err(Error::Jack)?;

		let (buffer_reader, buffer_writer) = RingBuffer::new(buffer_size)
			.map_err(|()| Error::RingBufferAllocFailure { size: buffer_size })?
			.into_reader_writer();
		let client = client.activate_async(
			AudioNotificationHandler::new(signals),
			AudioProcessHandler::new(buffer_writer),
		)
			.map_err(Error::Jack)?;
		Ok(AudioSourceController {
			client,
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
	// This should not need a Mutex, but it does.
	// https://github.com/RustAudio/rust-jack/issues/121
	ring_buffer: Mutex<RingBufferWriter>,
}

impl AudioProcessHandler {
	fn new(ring_buffer: RingBufferWriter) -> Self {
		AudioProcessHandler {
			ring_buffer: Mutex::new(ring_buffer),
		}
	}
}

impl ProcessHandler for AudioProcessHandler {
	fn process(&mut self, _: &Client, _process_scope: &ProcessScope) -> Control {
		Control::Continue
	}
}

impl JackSource for AudioSourceController {
	fn client(&self) -> &jack::Client {
		self.client.as_client()
	}

	fn source_type(&self) -> SourceType {
		SourceType::Audio
	}
}
