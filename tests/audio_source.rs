//! `AudioSource` against a real JACK server.
//!
//! The unit tests cover everything the audio module can be asked in isolation.
//! What is left is everything JACK itself decides: whether a client activates,
//! what the port list holds and when, whether a connection carries samples, and
//! what a server says as it dies. None of that has a stand-in worth writing —
//! a fake would only assert what this crate already believes.
//!
//! Every test starts a `jackd -d dummy` of its own through
//! [`test_support::jackd`](melody_visualizer::test_support::jackd), so the tests
//! need no server to be running and cannot disturb one that is. **Run them with
//! `cargo nextest run`**: each test process points `JACK_DEFAULT_SERVER` at its
//! own server, and a process has one environment to point.
//!
//! The dummy backend runs its periods off a wall clock, so nothing here asserts
//! on a sample count or an elapsed time. Each assertion is about content — a
//! port is listed, a window ascends by one — and waits for it to a deadline.

use std::thread::sleep;
use std::time::{Duration, Instant};

use async_channel::Receiver;
use jack::PortFlags;
use melody_visualizer::audio::source::{JackSource, PortName, SourceType, events::Event};
use melody_visualizer::audio::{AudioSource, CLIENT_NAME, SampleReader};
use melody_visualizer::test_support::jackd::{RampSource, Server};

/// The capture ring the app itself allocates: about 0.7 s of audio at 48 kHz.
const RING_BYTES: usize = 128 * 1024;

/// A ring far too small for one period, so that it overruns on every cycle.
const TINY_RING_BYTES: usize = 1024;

/// How long any one wait is given before the test fails.
const DEADLINE: Duration = Duration::from_secs(10);

/// How often a wait re-checks.
const POLL: Duration = Duration::from_millis(10);

const ACTIVATED: &str = "the test's own JACK server should accept the source";

/// Block until `condition` holds, or fail saying what never happened.
fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
	let deadline = Instant::now() + DEADLINE;
	while !condition() {
		assert!(
			Instant::now() < deadline,
			"timed out after {DEADLINE:?} waiting for {what}",
		);
		sleep(POLL);
	}
}

/// The next event `wanted` accepts, discarding whatever precedes it.
fn wait_for(events: &Receiver<Event>, what: &str, mut wanted: impl FnMut(&Event) -> bool) -> Event {
	let deadline = Instant::now() + DEADLINE;
	loop {
		match events.try_recv() {
			Ok(event) if wanted(&event) => return event,
			Ok(_) => continue,
			Err(_) => {
				assert!(
					Instant::now() < deadline,
					"timed out after {DEADLINE:?} waiting for {what}",
				);
				sleep(POLL);
			}
		}
	}
}

/// Discard the events reported so far, so the next one is the one under test.
fn discard_pending(events: &Receiver<Event>) {
	while events.try_recv().is_ok() {}
}

/// The source's own input port, by the name JACK knows it as.
fn input_port() -> PortName {
	PortName(format!("{CLIENT_NAME}:input"))
}

/// Every port the source registered, as a client other than the source sees it.
fn ports_of_the_source() -> Vec<String> {
	let (client, _) = jack::Client::new("port-probe", jack::ClientOptions::NO_START_SERVER)
		.expect("the test's JACK server should accept a probe client");
	client.ports(Some(&format!("^{CLIENT_NAME}:")), None, PortFlags::empty())
}

/// Whether every value in `window` is one more than the value before it.
///
/// This is what a counting ramp looks like once it has survived the whole
/// transport. A gap means samples were lost inside a window rather than between
/// two of them; a repeat means the reader read stale bytes; anything else means
/// the byte stream lost its alignment. Silence fails it too, which is what
/// distinguishes a connected port from a disconnected one.
fn ascends_by_one(window: &[f32]) -> bool {
	window.windows(2).all(|pair| pair[1] == pair[0] + 1.0)
}

/// Read windows until one is a full ramp, or fail with the last one read.
fn await_ramp(reader: &mut SampleReader, window: &mut [f32]) {
	let deadline = Instant::now() + DEADLINE;
	loop {
		let read = reader.read_latest(window);
		if read == window.len() && ascends_by_one(window) {
			return;
		}
		assert!(
			Instant::now() < deadline,
			"timed out after {DEADLINE:?} waiting for a contiguous run of samples; \
			 the last read returned {read} of {} samples, starting {:?}",
			window.len(),
			&window[..4.min(window.len())],
		);
		sleep(POLL);
	}
}

#[test]
fn a_source_registers_one_input_port_and_offers_the_server_s_own() {
	let _server = Server::start(44_100, 512);

	let (source, _reader, _events) = AudioSource::new(RING_BYTES).expect(ACTIVATED);

	assert_eq!(source.source_type(), SourceType::Audio);
	assert_eq!(
		ports_of_the_source(),
		[input_port().0],
		"the source registers exactly one port, and it is the input",
	);

	// Every JACK server has capture ports, and they are what the app connects to
	// its input. Its own port is not among them: an input cannot feed an input.
	let inputs = source.available_inputs();
	assert!(
		inputs
			.iter()
			.any(|port| port.as_str().starts_with("system:capture_")),
		"the server's capture ports should be offered as inputs, got {inputs:?}",
	);
	assert!(!inputs.contains(&input_port()));
}

#[test]
fn a_source_reads_its_sample_rate_from_the_server() {
	let _server = Server::start(44_100, 512);

	let (source, _reader, _events) = AudioSource::new(RING_BYTES).expect(ACTIVATED);

	assert_eq!(source.sample_rate(), 44_100);
}

/// The same assertion against a second rate, because one server proves only that
/// the number matches — two prove it was read rather than compiled in.
#[test]
fn a_source_reads_a_different_sample_rate_from_a_different_server() {
	let _server = Server::start(48_000, 1024);

	let (source, _reader, _events) = AudioSource::new(RING_BYTES).expect(ACTIVATED);

	assert_eq!(source.sample_rate(), 48_000);
}

#[test]
fn a_port_leaves_the_input_list_the_moment_it_is_unregistered() {
	let _server = Server::start(48_000, 512);
	let (source, _reader, events) = AudioSource::new(RING_BYTES).expect(ACTIVATED);

	let tone = RampSource::start("tone");
	let port = tone.port().clone();
	wait_until("the new client's port to be offered as an input", || {
		source.available_inputs().contains(&port)
	});

	discard_pending(&events);
	tone.stop();
	wait_for(&events, "the port list to change", |event| {
		matches!(event, Event::PortsChanged(_))
	});

	// No second look, no settling time. JACK goes on listing an unregistered
	// port for a few milliseconds (jack2#617) and the source subtracts it, so
	// the list is right at the instant the app is told to re-read it.
	assert!(
		!source.available_inputs().contains(&port),
		"an unregistered port is gone from the list as soon as the change is announced",
	);
}

#[test]
fn connecting_and_disconnecting_an_input_round_trips() {
	let _server = Server::start(48_000, 512);
	let (source, _reader, _events) = AudioSource::new(RING_BYTES).expect(ACTIVATED);

	let tone = RampSource::start("tone");
	wait_until("the tone's port to be offered as an input", || {
		source.available_inputs().contains(tone.port())
	});

	assert_eq!(source.connected_input(), None, "nothing feeds it yet");
	source.connect(tone.port()).expect("should connect");
	assert_eq!(source.connected_input().as_ref(), Some(tone.port()));
	source.disconnect().expect("should disconnect");
	assert_eq!(source.connected_input(), None);
}

#[test]
fn a_connection_made_by_another_client_is_reported_and_visible() {
	let _server = Server::start(48_000, 512);
	let (source, _reader, events) = AudioSource::new(RING_BYTES).expect(ACTIVATED);

	let tone = RampSource::start("tone");
	wait_until("the tone's port to be offered as an input", || {
		source.available_inputs().contains(tone.port())
	});
	discard_pending(&events);

	// The app is not involved: the other client wires itself up, as `jack_connect`
	// or any other JACK application would.
	tone.connect_to(&input_port());

	wait_for(&events, "the connection to be reported", |event| {
		matches!(event, Event::ConnectionChanged(_))
	});
	assert_eq!(
		source.connected_input().as_ref(),
		Some(tone.port()),
		"connection state is read back from the server, not remembered from what the app did",
	);
}

#[test]
fn samples_arrive_from_the_connected_port_unbroken() {
	let _server = Server::start(48_000, 512);
	let (source, mut reader, _events) = AudioSource::new(RING_BYTES).expect(ACTIVATED);

	let tone = RampSource::start("tone");
	wait_until("the tone's port to be offered as an input", || {
		source.available_inputs().contains(tone.port())
	});
	source.connect(tone.port()).expect("should connect");

	let mut window = [0.0; 256];
	await_ramp(&mut reader, &mut window);

	assert_eq!(reader.overruns(), 0, "the ring is far larger than a period");
}

#[test]
fn an_overrunning_ring_keeps_its_alignment() {
	let _server = Server::start(48_000, 512);
	let (source, mut reader, _events) = AudioSource::new(TINY_RING_BYTES).expect(ACTIVATED);

	let tone = RampSource::start("tone");
	wait_until("the tone's port to be offered as an input", || {
		source.available_inputs().contains(tone.port())
	});
	source.connect(tone.port()).expect("should connect");

	// The ring holds less than a period, so the real-time thread drops audio on
	// every cycle. What survives must still be whole samples in order: a partial
	// write would shift the byte stream and every value decoded after it.
	wait_until("the ring to overrun", || reader.overruns() > 0);

	let mut window = [0.0; 64];
	await_ramp(&mut reader, &mut window);
}

#[test]
fn a_server_that_dies_is_reported() {
	let mut server = Server::start(48_000, 512);
	let (_source, _reader, events) = AudioSource::new(RING_BYTES).expect(ACTIVATED);

	server.kill();

	let event = wait_for(&events, "the server's shutdown to be reported", |event| {
		matches!(event, Event::ServerShutdown(_))
	});
	let Event::ServerShutdown(shutdown) = event else {
		unreachable!("wait_for matched on the variant")
	};
	assert!(
		!shutdown.reason.is_empty(),
		"the reason JACK gave reaches the app",
	);
}
