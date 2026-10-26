use rtrb::{Consumer, Producer, RingBuffer};
use std::iter;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// The write end of the capture ring. Lives on the host's real-time audio thread.
pub struct SampleWriter {
	ring_buffer: Producer<f32>,
	overruns: Arc<AtomicU64>,
}

/// The read end of the capture ring, plus the overrun count the writer publishes.
///
/// ```
/// use melody_visualizer::audio::ring::sample_ring;
///
/// let (mut writer, mut reader) = sample_ring(1024);
/// writer.write_samples(&[1.0, 2.0, 3.0]);
///
/// let mut window = [0.0; 2];
/// assert_eq!(reader.read_latest(&mut window), 2);
/// assert_eq!(window, [2.0, 3.0], "the newest two samples, oldest first");
/// ```
pub struct SampleReader {
	buffer: Consumer<f32>,
	overruns: Arc<AtomicU64>,
}

/// Allocate a capture ring of `capacity` samples and pair its two ends over one
/// overrun counter.
pub fn sample_ring(capacity: usize) -> (SampleWriter, SampleReader) {
	let (ring_buffer, buffer) = RingBuffer::new(capacity);
	let overruns = Arc::new(AtomicU64::new(0));
	(
		SampleWriter {
			ring_buffer,
			overruns: overruns.clone(),
		},
		SampleReader { buffer, overruns },
	)
}

impl SampleWriter {
	/// Copy audio samples into the ring buffer.
	///
	/// Samples that do not fit are dropped and added to the overrun count. The
	/// reader skips ahead to the newest window on every tick, so it would never
	/// have read them anyway — the count is what tells the reader they existed,
	/// since dropped audio otherwise looks exactly like silence.
	pub fn write_samples(&mut self, samples: &[f32]) {
		// `slots` is a plain atomic read and the chunk writes into the ring in
		// place, so this stays real-time safe.
		let fits = samples.len().min(self.ring_buffer.slots());
		if let Ok(chunk) = self.ring_buffer.write_chunk_uninit(fits) {
			chunk.fill_from_iter(samples[..fits].iter().copied());
		}
		let dropped = samples.len() - fits;
		if dropped > 0 {
			// A lock-free read-modify-write: no allocation, no syscall, no wait.
			// Relaxed is enough — the count orders nothing but itself.
			self.overruns.fetch_add(dropped as u64, Ordering::Relaxed);
		}
	}
}

impl SampleReader {
	/// Copy the newest `out.len()` samples into `out`, oldest first, and return
	/// how many there were.
	///
	/// Anything older than that window is discarded: the consumer analyzes the
	/// sound as it is now, so audio it did not keep up with is of no use to it.
	/// The window itself stays in the ring, which is what lets consecutive calls
	/// overlap — each one asks for a whole window and gets the newest one,
	/// however little has arrived since the last.
	///
	/// A short return leaves the tail of `out` untouched; only `out[..n]` holds
	/// samples from this call.
	pub fn read_latest(&mut self, out: &mut [f32]) -> usize {
		let available = self.buffer.slots();
		if available > out.len()
			&& let Ok(stale) = self.buffer.read_chunk(available - out.len())
		{
			stale.commit_all();
		}

		let count = available.min(out.len());
		// Dropping the chunk without committing it leaves the window in the
		// ring. Its two slices are the two halves of a window that straddles
		// the wrap.
		if let Ok(window) = self.buffer.read_chunk(count) {
			let (first, second) = window.as_slices();
			for (destination, sample) in iter::zip(out.iter_mut(), first.iter().chain(second)) {
				*destination = *sample;
			}
		}
		count
	}

	/// Samples the real-time thread dropped, since the ring was allocated,
	/// because the ring was full when they arrived.
	pub fn overruns(&self) -> u64 {
		self.overruns.load(Ordering::Relaxed)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	/// Take every sample the ring holds, oldest first.
	fn drain(reader: &mut SampleReader) -> Vec<f32> {
		reader
			.buffer
			.read_chunk(reader.buffer.slots())
			.expect("it asks for what is there")
			.into_iter()
			.collect()
	}

	/// `count` samples counting up from zero.
	///
	/// A ramp rather than a constant because every property these tests assert —
	/// order, which window was returned — is invisible in a signal whose samples
	/// are all alike.
	fn ramp(count: usize) -> Vec<f32> {
		(0..count).map(|index| index as f32).collect()
	}

	#[test]
	fn write_samples_puts_them_in_the_ring_in_order() {
		let (mut writer, mut reader) = sample_ring(1024);

		let samples = [0.0f32, 1.0, -0.5, 123.456, f32::MIN, f32::MAX];
		writer.write_samples(&samples);

		assert_eq!(reader.overruns(), 0, "everything fit");
		assert_eq!(drain(&mut reader), samples);
	}

	#[test]
	fn a_write_that_exactly_fills_the_ring_drops_nothing() {
		let (mut writer, mut reader) = sample_ring(16);

		let samples = ramp(16);
		writer.write_samples(&samples);

		assert_eq!(reader.overruns(), 0, "every sample fit");
		assert_eq!(writer.ring_buffer.slots(), 0, "and there is no room left");
		assert_eq!(drain(&mut reader), samples);
	}

	#[test]
	fn write_samples_drops_what_does_not_fit_and_counts_it() {
		let (mut writer, mut reader) = sample_ring(16);

		let samples = ramp(16 + 3);
		writer.write_samples(&samples);

		assert_eq!(
			reader.overruns(),
			3,
			"only the samples that did not fit are counted as overruns",
		);
		assert_eq!(drain(&mut reader), samples[..16], "the oldest are kept");
	}

	#[test]
	fn overruns_accumulate_and_the_ring_takes_writes_again_once_read() {
		let (mut writer, mut reader) = sample_ring(16);

		// Two rounds of writing a full ring's worth into a ring nobody reads. The
		// first fills it; the second is dropped whole.
		let first = ramp(16);
		let second = first.iter().map(|sample| -sample).collect::<Vec<_>>();
		writer.write_samples(&first);
		writer.write_samples(&second);

		assert_eq!(
			reader.overruns(),
			16,
			"a write into a full ring is dropped in its entirety",
		);

		// Reading drains the ring, so the next write fits again.
		assert_eq!(drain(&mut reader), first);

		let third = first.iter().map(|sample| sample * 0.5).collect::<Vec<_>>();
		writer.write_samples(&third);

		assert_eq!(
			reader.overruns(),
			16,
			"a drained ring takes the next block without dropping",
		);
		assert_eq!(drain(&mut reader), third);
	}

	// --- Window reads ------------------------------------------------------

	#[test]
	fn read_latest_takes_what_little_there_is_and_says_how_much() {
		let (mut writer, mut reader) = sample_ring(1024);
		writer.write_samples(&ramp(3));

		let mut window = [-1.0; 8];
		assert_eq!(reader.read_latest(&mut window), 3);

		assert_eq!(window[..3], [0.0, 1.0, 2.0]);
		assert_eq!(
			window[3..],
			[-1.0; 5],
			"a short read leaves the rest of the window untouched",
		);
	}

	#[test]
	fn read_latest_fills_a_window_the_ring_holds_exactly() {
		let (mut writer, mut reader) = sample_ring(1024);
		let samples = ramp(8);
		writer.write_samples(&samples);

		let mut window = [0.0; 8];
		assert_eq!(reader.read_latest(&mut window), 8);

		assert_eq!(window[..], samples[..]);
	}

	#[test]
	fn read_latest_returns_the_newest_window_and_discards_what_is_older() {
		let (mut writer, mut reader) = sample_ring(1024);
		writer.write_samples(&ramp(100));

		let mut window = [0.0; 4];
		assert_eq!(reader.read_latest(&mut window), 4);
		assert_eq!(window, [96.0, 97.0, 98.0, 99.0], "the four newest samples");

		// The window itself stays in the ring, so a consumer reading faster than
		// the audio arrives sees overlapping windows rather than empty ones.
		assert_eq!(reader.read_latest(&mut window), 4);
		assert_eq!(window, [96.0, 97.0, 98.0, 99.0]);
	}

	#[test]
	fn read_latest_reads_a_window_that_straddles_the_end_of_the_ring() {
		let (mut writer, mut reader) = sample_ring(16);

		// Fill the ring and take a small window off it. That leaves the read
		// position near the end of the buffer, so the next block written wraps
		// round to the start.
		writer.write_samples(&ramp(16));
		assert_eq!(reader.read_latest(&mut [0.0; 4]), 4);

		writer.write_samples(&ramp(16 + 12)[16..]);
		assert_eq!(
			reader.overruns(),
			0,
			"the refill was sized to what was free"
		);
		assert!(
			!reader
				.buffer
				.read_chunk(16)
				.unwrap()
				.as_slices()
				.1
				.is_empty(),
			"the readable samples should span the end of the ring",
		);

		let mut window = [0.0; 16];
		assert_eq!(reader.read_latest(&mut window), 16);

		assert_eq!(
			window[..],
			ramp(16 + 12)[12..],
			"a window across the wrap reads in order",
		);
	}
}
