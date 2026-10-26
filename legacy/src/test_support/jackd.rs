//! A private JACK server for one test, and a client to drive it with.
//!
//! Tests that touch [`AudioSource`](crate::audio::AudioSource) need a real
//! server, and they must not share one: a port another test registers, or a
//! server another test kills, is a failure with no cause in the test that
//! reports it. [`Server`] gives each test its own `jackd -d dummy`, on a
//! backend that touches no hardware and needs no realtime scheduling.
//!
//! ```no_run
//! use melody_visualizer::test_support::jackd::{RampSource, Server};
//!
//! let _server = Server::start(48_000, 512);
//! let tone = RampSource::start("tone");
//! assert!(tone.port().as_str().starts_with("tone:"));
//! ```

use jack::{AudioOut, Client, ClientOptions, Control, Port, ProcessHandler, ProcessScope};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use crate::audio::source::PortName;

/// How long `jackd` gets to come up and accept its first client.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// How often the readiness check retries while waiting for that.
const STARTUP_POLL: Duration = Duration::from_millis(20);

/// How long `jackd` gets to clean up after being asked to stop.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Whether this process has already claimed a server.
///
/// Which server a JACK client opens comes from the `JACK_DEFAULT_SERVER`
/// environment variable, and a process has exactly one environment. See
/// [`Server::start`].
static SERVER_CLAIMED: AtomicBool = AtomicBool::new(false);

/// A `jackd -d dummy` of this test's own, killed when the test ends.
///
/// Hold it for as long as the clients that talk to it: dropping it kills the
/// server, which is how the shutdown path is exercised deliberately and how it
/// is exercised by accident if the binding is dropped too early.
pub struct Server {
	name: String,
	child: Child,
}

impl Server {
	/// Start a dummy server running at `sample_rate` frames per second, in
	/// periods of `period` frames, and wait until it accepts clients.
	///
	/// Every JACK client in this process reaches it afterwards, because
	/// `start` points `JACK_DEFAULT_SERVER` at it. `ClientOptions::SERVER_NAME`
	/// is the interface meant for this and cannot be used: `Client::new` calls
	/// `jack_client_open` without the varargs the flag reads the name from, so
	/// setting the bit names no server at all.
	///
	/// # Panics
	/// - if this process already started a server, since the second one could
	///   only be reached by changing the environment out from under the first.
	///   Run the suite with `cargo nextest run`, which gives every test its own
	///   process.
	/// - if `jackd` is not installed, or does not accept a client in time.
	pub fn start(sample_rate: u32, period: u32) -> Server {
		assert!(
			!SERVER_CLAIMED.swap(true, Ordering::SeqCst),
			"a process can hold one JACK server, because it has one \
			 JACK_DEFAULT_SERVER; run the tests with `cargo nextest run`",
		);

		let name = format!("melody-test-{}", std::process::id());
		// SAFETY: this runs before the test has started a thread of its own, and
		// before any JACK client exists to have read the variable.
		unsafe { std::env::set_var("JACK_DEFAULT_SERVER", &name) };

		let child = Command::new("jackd")
			// The sandbox cannot grant realtime scheduling, and the server
			// refuses to start rather than fall back on its own.
			.args(["-n", &name, "-r", "-d", "dummy"])
			.args(["-r", &sample_rate.to_string()])
			.args(["-p", &period.to_string()])
			// The dummy backend claims no device, so there is nothing to reserve
			// and no D-Bus session to reserve it through.
			.env("JACK_NO_AUDIO_RESERVATION", "1")
			.stdout(Stdio::null())
			.stderr(Stdio::null())
			.spawn()
			.expect("`jackd` should be installed — `apt install jackd2`");

		let server = Server { name, child };
		server.await_ready();
		server
	}

	/// The server's name, as `JACK_DEFAULT_SERVER` names it.
	pub fn name(&self) -> &str {
		&self.name
	}

	/// Stop the server, which is what its clients see as the server going away.
	///
	/// Dropping the `Server` does the same; this is for a test that wants the
	/// death itself rather than the cleanup.
	///
	/// `SIGTERM` rather than `SIGKILL`, and then a wait: a JACK server that is
	/// killed outright leaves its shared-memory segments behind, and enough of
	/// those fill the registry every later server would have to register in.
	/// That failure lands on a test that ran long afterwards.
	pub fn kill(&mut self) {
		let _ = Command::new("kill")
			.arg(self.child.id().to_string())
			.status();

		let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
		while Instant::now() < deadline {
			match self.child.try_wait() {
				Ok(Some(_)) => return,
				_ => sleep(STARTUP_POLL),
			}
		}

		let _ = self.child.kill();
		let _ = self.child.wait();
	}

	/// Block until the server accepts a client, or panic on the deadline.
	///
	/// Readiness is the server answering, not a fixed wait: `jackd` writes
	/// nothing to say it is up, and a sleep long enough to be safe on a loaded
	/// machine is one that costs every run.
	fn await_ready(&self) {
		let deadline = Instant::now() + STARTUP_TIMEOUT;
		while Instant::now() < deadline {
			if Client::new("readiness-probe", ClientOptions::NO_START_SERVER).is_ok() {
				return;
			}
			sleep(STARTUP_POLL);
		}
		panic!(
			"`jackd` did not accept a client on server {} within {:?}",
			self.name, STARTUP_TIMEOUT,
		);
	}
}

impl Drop for Server {
	fn drop(&mut self) {
		self.kill();
	}
}

/// A second JACK client, feeding one output port a counting ramp.
///
/// The ramp is what makes the samples say where they came from: every value is
/// one more than the last, so a gap, a repeat or a byte-misaligned decode shows
/// up in the numbers. A sine or a constant would read as plausible audio however
/// badly the transport mangled it.
pub struct RampSource {
	client: jack::AsyncClient<(), Ramp>,
	port: PortName,
}

impl RampSource {
	/// Register `name` as a JACK client with one output port, and start it.
	///
	/// # Panics
	/// - if no JACK server is running, which for a test means [`Server::start`]
	///   was not called or its `Server` was dropped.
	pub fn start(name: &str) -> RampSource {
		let (client, _) = Client::new(name, ClientOptions::NO_START_SERVER)
			.expect("the test's JACK server should accept a second client");
		let port = client
			.register_port("output", AudioOut::default())
			.expect("a fresh client has room for one output port");
		let port_name = PortName(port.name().expect("a registered port has a name"));

		let client = client
			.activate_async((), Ramp { port, next: 0.0 })
			.expect("the client should activate");
		RampSource {
			client,
			port: port_name,
		}
	}

	/// The fully-qualified name of the port the ramp comes out of.
	pub fn port(&self) -> &PortName {
		&self.port
	}

	/// Feed `port` from this client's output, as any other JACK client would.
	///
	/// This is the connection the app did not make, and the one it has to notice
	/// all the same.
	pub fn connect_to(&self, port: &PortName) {
		self.client
			.as_client()
			.connect_ports_by_name(self.port.as_str(), port.as_str())
			.expect("both ports should exist and accept the connection");
	}

	/// Close the client, which unregisters its port.
	pub fn stop(self) {
		let _ = self.client.deactivate();
	}
}

/// Writes a counting ramp into an output port, one value per frame.
struct Ramp {
	port: Port<AudioOut>,
	next: f32,
}

impl ProcessHandler for Ramp {
	fn process(&mut self, _client: &Client, scope: &ProcessScope) -> Control {
		for sample in self.port.as_mut_slice(scope) {
			*sample = self.next;
			self.next += 1.0;
		}
		Control::Continue
	}
}
