use jack::{
	AudioIn, Client, ClientStatus, Control, Frames, NotificationHandler, Port, PortId,
	ProcessHandler, ProcessScope, RingBuffer, RingBufferReader, RingBufferWriter,
};
use log::error;
use std::sync::Mutex;

use crate::error::Error;
use crate::pubsub::Notifier;
use crate::source::{JackSource, SourceType, events};

const TITLE: &str = "Melody Visualizer";

pub struct AudioSourceController {
	client: jack::AsyncClient<AudioNotificationHandler, AudioProcessHandler>,
	input_port: jack::Port<jack::Unowned>,
}

impl AudioSourceController {
	pub fn new(buffer_size: usize, notifier: Notifier) -> Result<(Self, RingBufferReader), Error> {
		let (client, status) =
			jack::Client::new(TITLE, jack::ClientOptions::NO_START_SERVER).map_err(Error::Jack)?;
		if !status.is_empty() {
			return Err(Error::JackStatus(status));
		}

		let port = client
			.register_port("input", AudioIn::default())
			.map_err(Error::Jack)?;
		let input_port = port.clone_unowned();

		let (buffer_reader, buffer_writer) = RingBuffer::new(buffer_size)
			.map_err(|_| Error::RingBufferAllocFailure { size: buffer_size })?
			.into_reader_writer();
		let client = client
			.activate_async(
				AudioNotificationHandler::new(notifier),
				AudioProcessHandler::new(port, buffer_writer),
			)
			.map_err(Error::Jack)?;
		let controller = AudioSourceController { client, input_port };
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

	// The methods below hold the event-forwarding logic, free of any JACK runtime
	// types (`&Client`), so they can be unit-tested without a live JACK server.
	// The `NotificationHandler` trait impl is a thin wrapper around them.

	fn notify_sample_rate(&self, sample_rate: Frames) {
		if let Err(err) = self.notifier.send(events::SampleRateChanged(sample_rate)) {
			error!("failed to notify of JACK sample rate change: {}", err);
		}
	}

	fn notify_port_registration(&self, port_id: PortId, is_registered: bool) {
		let notification = if is_registered {
			events::InputsChanged::Registered(port_id)
		} else {
			events::InputsChanged::Unregistered(port_id)
		};
		if let Err(err) = self.notifier.send(notification) {
			error!("failed to notify of JACK input change: {}", err);
		}
	}

	fn notify_port_rename(&self, port_id: PortId, new_name: &str) {
		let notification = events::InputsChanged::Renamed(port_id, new_name.into());
		if let Err(err) = self.notifier.send(notification) {
			error!("failed to notify of JACK input change: {}", err);
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

	fn port_registration(&mut self, _client: &Client, port_id: PortId, is_registered: bool) {
		self.notify_port_registration(port_id, is_registered);
	}

	fn port_rename(
		&mut self,
		_: &Client,
		port_id: PortId,
		_old_name: &str,
		new_name: &str,
	) -> Control {
		self.notify_port_rename(port_id, new_name);
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
		let mut ring_buffer = self
			.ring_buffer
			.lock()
			.expect("I shouldn't even need a Mutex...");
		// Samples that do not fit are dropped whole. The reader skips ahead to the
		// newest window on every tick, so it would never have read them anyway.
		let _dropped = write_samples(&mut ring_buffer, self.port.as_slice(scope));
		Control::Continue
	}
}

/// Copy audio samples into the ring buffer as native-endian bytes.
///
/// The ring holds whole samples only. `RingBufferWriter::write_buffer` writes as
/// much as fits and returns short, so a sample is written only once the ring has
/// room for all four of its bytes; the rest are dropped. A partial sample would
/// shift the byte stream and every value the reader decodes after it.
///
/// Returns the number of samples dropped for want of space.
///
/// Pulled out of `AudioProcessHandler::process` so it can be unit-tested without
/// a live JACK `ProcessScope` / `Port`.
fn write_samples(ring_buffer: &mut RingBufferWriter, samples: &[f32]) -> usize {
	// TODO: Create a custom ring buffer holding an [f32] that is more efficient.
	let mut dropped = 0;
	for sample in samples {
		// `space` is a plain atomic read, so this stays real-time safe.
		if ring_buffer.space() < size_of::<f32>() {
			dropped += 1;
			continue;
		}
		ring_buffer.write_buffer(&sample.to_ne_bytes());
	}
	dropped
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

#[cfg(test)]
mod tests {
	use super::*;
	use crate::pubsub::PubSub;
	use crate::source::events::{InputsChanged, SampleRateChanged};
	use crate::test_support::run_in_glib_main_loop;
	use futures::prelude::*;
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

			let handler = AudioNotificationHandler::new(pubsub.notifier());
			handler.notify_sample_rate(48_000);
			yield_rx.next().await.unwrap();

			assert_eq!(*received.borrow(), Some(SampleRateChanged(48_000)));
		});
	}

	#[test]
	fn notification_handler_forwards_port_registration() {
		run_in_glib_main_loop(|mut yield_rx| async move {
			let pubsub = PubSub::new(None, glib::Priority::DEFAULT);
			let received = Rc::new(RefCell::new(Vec::new()));

			let received_clone = received.clone();
			let _handle = pubsub.subscribe(move |event: &InputsChanged| {
				received_clone.borrow_mut().push(event.clone());
			});

			let handler = AudioNotificationHandler::new(pubsub.notifier());
			handler.notify_port_registration(7, true);
			handler.notify_port_registration(7, false);
			yield_rx.next().await.unwrap();

			assert_eq!(
				*received.borrow(),
				vec![InputsChanged::Registered(7), InputsChanged::Unregistered(7)],
			);
		});
	}

	#[test]
	fn notification_handler_forwards_port_rename() {
		run_in_glib_main_loop(|mut yield_rx| async move {
			let pubsub = PubSub::new(None, glib::Priority::DEFAULT);
			let received = Rc::new(RefCell::new(None));

			let received_clone = received.clone();
			let _handle = pubsub.subscribe(move |event: &InputsChanged| {
				*received_clone.borrow_mut() = Some(event.clone());
			});

			let handler = AudioNotificationHandler::new(pubsub.notifier());
			handler.notify_port_rename(3, "system:capture_9");
			yield_rx.next().await.unwrap();

			assert_eq!(
				*received.borrow(),
				Some(InputsChanged::Renamed(3, "system:capture_9".to_string())),
			);
		});
	}

	// --- Ring buffer writes -----------------------------------------------

	#[test]
	fn process_handler_writes_samples_to_ring_buffer() {
		let (mut reader, mut writer) = RingBuffer::new(1024)
			.expect("ring buffer allocation is a userspace operation, needs no JACK server")
			.into_reader_writer();

		let samples = [0.0f32, 1.0, -0.5, 123.456, f32::MIN, f32::MAX];
		assert_eq!(write_samples(&mut writer, &samples), 0);

		// Samples are serialized as native-endian f32 bytes, 4 bytes each.
		let mut bytes = vec![0u8; samples.len() * 4];
		let n = reader.read_buffer(&mut bytes);
		assert_eq!(n, bytes.len(), "all sample bytes should be readable back");

		let read_back: Vec<f32> = bytes
			.chunks_exact(4)
			.map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
			.collect();
		assert_eq!(read_back, samples);
	}

	#[test]
	fn write_samples_drops_whole_samples_when_the_ring_fills() {
		let (mut reader, mut writer) = RingBuffer::new(64)
			.expect("ring buffer allocation is a userspace operation, needs no JACK server")
			.into_reader_writer();

		// JACK sizes the ring to a power of two and keeps one byte free, so the
		// capacity is not a whole number of samples: the last few bytes are exactly
		// the gap a short write would split an f32 across.
		let capacity = writer.space();
		let fits = capacity / size_of::<f32>();
		let overflow = 3;

		let samples = (0..(fits + overflow) as u32)
			.map(|i| i as f32)
			.collect::<Vec<_>>();
		let dropped = write_samples(&mut writer, &samples);

		assert_eq!(
			dropped, overflow,
			"only the samples that did not fit are dropped"
		);
		assert_eq!(
			writer.space(),
			capacity - fits * size_of::<f32>(),
			"the bytes too few to hold a sample are left unwritten",
		);

		let mut bytes = vec![0u8; capacity];
		let n = reader.read_buffer(&mut bytes);
		assert_eq!(n % size_of::<f32>(), 0, "the ring holds only whole samples");

		let read_back: Vec<f32> = bytes[..n]
			.chunks_exact(4)
			.map(|c| f32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
			.collect();
		assert_eq!(
			read_back,
			samples[..fits],
			"the reader decodes in alignment"
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
	}
}
