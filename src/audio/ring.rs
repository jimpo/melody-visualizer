use itertools::Itertools;
use jack::{RingBuffer, RingBufferReader, RingBufferWriter};
use std::iter;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::Error;

/// The write end of the capture ring. Lives on the JACK real-time thread.
///
/// Kept apart from `AudioProcessHandler` so it can be unit-tested without a live
/// JACK `ProcessScope` / `Port`: allocating a `RingBuffer` needs no JACK server.
pub struct SampleWriter {
	ring_buffer: RingBufferWriter,
	overruns: Arc<AtomicU64>,
}

/// The read end of the capture ring, plus the overrun count the writer publishes.
///
/// The samples are bytes on the wire and `f32`s at the seam: decoding them is
/// this type's job, so nothing downstream has to know the ring holds bytes at
/// all.
///
/// ```
/// use melody_visualizer::audio::ring::sample_ring;
///
/// let (mut writer, mut reader) = sample_ring(1024)?;
/// writer.write_samples(&[1.0, 2.0, 3.0]);
///
/// let mut window = [0.0; 2];
/// assert_eq!(reader.read_latest(&mut window), 2);
/// assert_eq!(window, [2.0, 3.0], "the newest two samples, oldest first");
/// # Ok::<(), melody_visualizer::error::Error>(())
/// ```
pub struct SampleReader {
	buffer: RingBufferReader,
	overruns: Arc<AtomicU64>,
}

/// Allocate the capture ring and pair its two ends over one overrun counter.
pub fn sample_ring(size: usize) -> Result<(SampleWriter, SampleReader), Error> {
	let (buffer, ring_buffer) = RingBuffer::new(size)
		.map_err(|_| Error::RingBufferAllocFailure { size })?
		.into_reader_writer();
	let overruns = Arc::new(AtomicU64::new(0));
	Ok((
		SampleWriter {
			ring_buffer,
			overruns: overruns.clone(),
		},
		SampleReader { buffer, overruns },
	))
}

impl SampleWriter {
	/// Copy audio samples into the ring buffer as native-endian bytes.
	///
	/// The ring holds whole samples only. `RingBufferWriter::write_buffer` writes
	/// as much as fits and returns short, so a sample is written only once the
	/// ring has room for all four of its bytes; the rest are dropped. A partial
	/// sample would shift the byte stream and every value the reader decodes
	/// after it.
	///
	/// Samples that do not fit are dropped whole and added to the overrun count.
	/// The reader skips ahead to the newest window on every tick, so it would
	/// never have read them anyway — the count is what tells the reader they
	/// existed, since dropped audio otherwise looks exactly like silence.
	pub fn write_samples(&mut self, samples: &[f32]) {
		// TODO: Create a custom ring buffer holding an [f32] that is more efficient.
		let mut dropped = 0;
		for sample in samples {
			// `space` is a plain atomic read, so this stays real-time safe.
			if self.ring_buffer.space() < size_of::<f32>() {
				dropped += 1;
				continue;
			}
			self.ring_buffer.write_buffer(&sample.to_ne_bytes());
		}
		if dropped > 0 {
			// A lock-free read-modify-write: no allocation, no syscall, no wait.
			// Relaxed is enough — the count orders nothing but itself.
			self.overruns.fetch_add(dropped, Ordering::Relaxed);
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
		let wanted = size_of_val(out);
		let available = self.buffer.space();
		if available > wanted {
			self.buffer.advance(available - wanted);
		}

		let count = available.min(wanted) / size_of::<f32>();
		// `peek_iter` chains the two halves of the ring, so a window that
		// straddles the wrap reads as one run of bytes. The writer only ever
		// commits whole samples, so grouping them in fours stays in step.
		let samples = self
			.buffer
			.peek_iter()
			.take(count * size_of::<f32>())
			.tuples::<(_, _, _, _)>()
			.map(|(&b1, &b2, &b3, &b4)| f32::from_ne_bytes([b1, b2, b3, b4]));
		for (destination, sample) in iter::zip(out.iter_mut(), samples) {
			*destination = sample;
		}
		count
	}

	/// Samples the real-time thread dropped, since the client was activated,
	/// because the ring was full when they arrived.
	pub fn overruns(&self) -> u64 {
		self.overruns.load(Ordering::Relaxed)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const RING_ALLOC_FAILED: &str =
		"ring buffer allocation is a userspace operation, needs no JACK server";

	/// Decode a ring buffer's bytes back into the samples the writer put in.
	fn read_samples(reader: &mut RingBufferReader, capacity: usize) -> Vec<f32> {
		let mut bytes = vec![0u8; capacity];
		let n = reader.read_buffer(&mut bytes);
		assert_eq!(n % size_of::<f32>(), 0, "the ring holds only whole samples");
		bytes[..n]
			.as_chunks::<{ size_of::<f32>() }>()
			.0
			.iter()
			.map(|&c| f32::from_ne_bytes(c))
			.collect()
	}

	#[test]
	fn process_handler_writes_samples_to_ring_buffer() {
		let (mut writer, mut reader) = sample_ring(1024).expect(RING_ALLOC_FAILED);

		let samples = [0.0f32, 1.0, -0.5, 123.456, f32::MIN, f32::MAX];
		writer.write_samples(&samples);

		assert_eq!(reader.overruns(), 0, "everything fit");
		// Samples are serialized as native-endian f32 bytes, 4 bytes each.
		assert_eq!(
			read_samples(&mut reader.buffer, samples.len() * size_of::<f32>()),
			samples,
		);
	}

	#[test]
	fn write_samples_drops_whole_samples_when_the_ring_fills() {
		let (mut writer, mut reader) = sample_ring(64).expect(RING_ALLOC_FAILED);

		// JACK sizes the ring to a power of two and keeps one byte free, so the
		// capacity is not a whole number of samples: the last few bytes are exactly
		// the gap a short write would split an f32 across.
		let capacity = writer.ring_buffer.space();
		let fits = capacity / size_of::<f32>();
		let overflow = 3;

		let samples = (0..(fits + overflow) as u32)
			.map(|i| i as f32)
			.collect::<Vec<_>>();
		writer.write_samples(&samples);

		assert_eq!(
			reader.overruns(),
			overflow as u64,
			"only the samples that did not fit are counted as overruns",
		);
		assert_eq!(
			writer.ring_buffer.space(),
			capacity - fits * size_of::<f32>(),
			"the bytes too few to hold a sample are left unwritten",
		);
		assert_eq!(
			read_samples(&mut reader.buffer, capacity),
			samples[..fits],
			"the reader decodes in alignment",
		);
	}

	#[test]
	fn overruns_accumulate_and_the_ring_realigns_once_reading_resumes() {
		let (mut writer, mut reader) = sample_ring(64).expect(RING_ALLOC_FAILED);

		let capacity = writer.ring_buffer.space();
		let fits = capacity / size_of::<f32>();

		// Two rounds of writing a full ring's worth into a ring nobody reads. The
		// first fills it; the second is dropped whole.
		let first = (0..fits as u32).map(|i| i as f32).collect::<Vec<_>>();
		let second = (0..fits as u32).map(|i| -(i as f32)).collect::<Vec<_>>();
		writer.write_samples(&first);
		let after_first = reader.overruns();
		writer.write_samples(&second);

		assert_eq!(
			reader.overruns() - after_first,
			fits as u64,
			"a write into a full ring is dropped in its entirety",
		);

		// Reading drains the ring, so the next write fits again — and every sample
		// read back is one that was written whole, in order.
		assert_eq!(read_samples(&mut reader.buffer, capacity), first);

		let third = (0..fits as u32).map(|i| i as f32 * 0.5).collect::<Vec<_>>();
		let before_third = reader.overruns();
		writer.write_samples(&third);

		assert_eq!(
			reader.overruns(),
			before_third,
			"a drained ring takes the next block without dropping",
		);
		assert_eq!(
			read_samples(&mut reader.buffer, capacity),
			third,
			"the byte stream stays sample-aligned across an overrun",
		);
	}
}
