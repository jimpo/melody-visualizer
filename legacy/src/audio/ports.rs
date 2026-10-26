use jack::{AudioOut, Client, PortFlags, PortSpec};
use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use super::source::PortName;

/// The JACK ports that can feed an audio input: every audio port the server
/// reads out of, minus the ones it has announced as gone.
pub fn audio_outputs(client: &Client, retired: &RetiredPorts) -> Vec<PortName> {
	let listed = client.ports(
		None,
		Some(AudioOut::default().jack_port_type()),
		PortFlags::IS_OUTPUT,
	);
	retired.subtract(listed)
}

/// The ports JACK has announced as unregistered but still lists.
///
/// `jack_get_ports` reads a client-side copy of the graph that lags the
/// port-registration callback, so a port that has just gone away keeps being
/// listed for a few milliseconds after the notification
/// ([jack2#617](https://github.com/jackaudio/jack2/issues/617)). The
/// notification thread records the name here and [`audio_outputs`] subtracts
/// it, which makes the list right at the instant the notification arrives.
#[derive(Clone, Default)]
pub struct RetiredPorts(Arc<Mutex<HashSet<String>>>);

impl RetiredPorts {
	/// Record a port JACK has announced as unregistered.
	pub fn retire(&self, name: String) {
		self.names().insert(name);
	}

	/// Take a port back into the list, because JACK registered the name again.
	pub fn restore(&self, name: &str) {
		self.names().remove(name);
	}

	/// Drop the retired ports from `listed`.
	///
	/// A retired name that `listed` no longer holds is forgotten: JACK's copy
	/// of the graph has caught up, and the name is free to be registered again.
	pub(super) fn subtract(&self, listed: Vec<String>) -> Vec<PortName> {
		let mut retired = self.names();
		retired.retain(|name| listed.contains(name));
		listed
			.into_iter()
			.filter(|name| !retired.contains(name))
			.map(PortName::from)
			.collect()
	}

	/// The set, recovered from a poisoned lock. A panic on the notification
	/// thread must not take the port list down with it: the worst a stale set
	/// costs is a port shown or hidden one refresh too long.
	fn names(&self) -> MutexGuard<'_, HashSet<String>> {
		self.0.lock().unwrap_or_else(|err| err.into_inner())
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn listed(names: &[&str]) -> Vec<String> {
		names.iter().map(|name| name.to_string()).collect()
	}

	fn names(ports: Vec<PortName>) -> Vec<String> {
		ports.into_iter().map(|port| port.0).collect()
	}

	#[test]
	fn a_retired_port_is_hidden_while_jack_still_lists_it() {
		let retired = RetiredPorts::default();
		retired.retire("tone:output1".to_string());

		assert_eq!(
			names(retired.subtract(listed(&["system:capture_1", "tone:output1"]))),
			["system:capture_1"],
			"the port is gone from the list the moment JACK says it was unregistered",
		);
	}

	#[test]
	fn a_retired_port_is_forgotten_once_jack_stops_listing_it() {
		let retired = RetiredPorts::default();
		retired.retire("tone:output1".to_string());
		retired.subtract(listed(&["system:capture_1", "tone:output1"]));

		// JACK's copy of the graph catches up.
		assert_eq!(
			names(retired.subtract(listed(&["system:capture_1"]))),
			["system:capture_1"],
		);
		// So a port registered later under the same name is listed again.
		assert_eq!(
			names(retired.subtract(listed(&["system:capture_1", "tone:output1"]))),
			["system:capture_1", "tone:output1"],
		);
	}

	#[test]
	fn a_port_registered_again_under_its_old_name_comes_back() {
		let retired = RetiredPorts::default();
		retired.retire("tone:output1".to_string());
		retired.restore("tone:output1");

		assert_eq!(
			names(retired.subtract(listed(&["system:capture_1", "tone:output1"]))),
			["system:capture_1", "tone:output1"],
			"a name JACK registered again is listed even before its removal settles",
		);
	}
}
