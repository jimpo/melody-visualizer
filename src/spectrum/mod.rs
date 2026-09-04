pub mod generators;
pub mod renderer;
pub mod transforms;

use std::{
	any::Any,
	fmt::{self, Debug},
	sync::Arc,
	time::Duration,
};

pub type Hz = f64;
pub type LogHz = f64;

#[derive(Debug, Clone)]
pub struct SpectrumBuffer {
	params: Arc<SpectrumParams>,
	data: Vec<f64>,
}

impl SpectrumBuffer {
	pub fn new(params: Arc<SpectrumParams>) -> Self {
		let data = vec![0.0; params.samples()];
		SpectrumBuffer { params, data }
	}

	pub fn fill(mut self, f: impl Fn(&mut [f64], &SpectrumParams)) -> Spectrum {
		f(&mut self.data, &self.params);
		Spectrum { buffer: self }
	}

	pub fn params(&self) -> &Arc<SpectrumParams> {
		&self.params
	}

	/// Moves the buffer onto `params`, keeping its allocation.
	///
	/// The values do not survive: they belong to the grid the buffer is leaving.
	/// This is how a buffer built on a stale grid re-enters the recycling ring
	/// instead of being dropped for a freshly allocated one.
	///
	/// # Examples
	///
	/// ```
	/// use std::sync::Arc;
	/// use melody_visualizer::spectrum::{SpectrumBuffer, SpectrumParams};
	///
	/// let coarse = Arc::new(SpectrumParams::exp_spaced(4, 200.0, 1600.0));
	/// let fine = Arc::new(SpectrumParams::exp_spaced(8, 200.0, 1600.0));
	///
	/// let buffer = SpectrumBuffer::new(coarse).regrid(fine.clone());
	///
	/// assert!(Arc::ptr_eq(buffer.params(), &fine));
	/// assert_eq!(buffer.fill(|_values, _params| {}).values(), [0.0; 8]);
	/// ```
	pub fn regrid(mut self, params: Arc<SpectrumParams>) -> Self {
		self.data.clear();
		self.data.resize(params.samples(), 0.0);
		self.params = params;
		self
	}
}

impl Default for SpectrumBuffer {
	fn default() -> Self {
		Self::new(Arc::new(SpectrumParams::default()))
	}
}

#[derive(Clone, Default)]
pub struct Spectrum {
	buffer: SpectrumBuffer,
}

impl Spectrum {
	pub fn into_buffer(self) -> SpectrumBuffer {
		self.buffer
	}

	pub fn values(&self) -> &[f64] {
		&self.buffer.data
	}
	pub fn values_mut(&mut self) -> &mut [f64] {
		&mut self.buffer.data
	}
	pub fn params(&self) -> &Arc<SpectrumParams> {
		self.buffer.params()
	}
}

#[derive(Clone, Default, PartialEq)]
pub struct SpectrumParams {
	frequencies: Vec<f64>,
	log_frequencies: Vec<f64>,
}

impl SpectrumParams {
	pub fn exp_spaced(samples: usize, min_freq: f64, max_freq: f64) -> Self {
		// there must be at least two samples, one at min_freq and one at max_freq
		assert!(samples >= 2);

		let mut frequencies = Vec::with_capacity(samples);
		let mut log_frequencies = Vec::with_capacity(samples);

		let min_log_freq = min_freq.log2();
		let max_log_freq = max_freq.log2();
		for i in 0..samples {
			let interp_ratio = (i as f64) / ((samples - 1) as f64);
			let log_freq = min_log_freq * (1.0 - interp_ratio) + max_log_freq * interp_ratio;
			let freq = log_freq.exp2();

			frequencies.push(freq);
			log_frequencies.push(log_freq);
		}
		SpectrumParams {
			frequencies,
			log_frequencies,
		}
	}

	pub fn samples(&self) -> usize {
		self.frequencies.len()
	}

	pub fn frequencies(&self) -> &[f64] {
		&self.frequencies
	}

	pub fn log_frequencies(&self) -> &[f64] {
		&self.log_frequencies
	}

	pub fn min_freq(&self) -> Option<f64> {
		self.frequencies.first().cloned()
	}

	pub fn max_freq(&self) -> Option<f64> {
		self.frequencies.last().cloned()
	}

	pub fn min_log_freq(&self) -> Option<f64> {
		self.log_frequencies.first().cloned()
	}

	pub fn max_log_freq(&self) -> Option<f64> {
		self.log_frequencies.last().cloned()
	}
}

impl fmt::Debug for SpectrumParams {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.debug_struct("SpectrumParams")
			.field("min_freq", &self.min_freq())
			.field("max_freq", &self.max_freq())
			.field("samples", &self.samples())
			.finish()
	}
}

pub trait SpectrumGenerator: Debug + Send {
	fn generate(&mut self, buffer: SpectrumBuffer) -> Spectrum;
	fn interval(&self) -> Duration;

	/// Updates the rate of the samples feeding this generator.
	///
	/// Generators which do not consume sampled input can ignore it.
	fn set_sample_rate(&mut self, _sample_rate: u32) {}
}

/// Identifies a transform across the `Config` that describes it, the
/// [`TransformChain`] that runs it, and the GUI control that edits it.
#[derive(
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	Hash,
	PartialOrd,
	Ord,
	derive_more::Display,
	derive_more::From,
)]
pub struct TransformId(pub u64);

pub trait SpectrumTransform: Debug + Send {
	/// Applies the transform to `spectrum`.
	///
	/// # Preconditions
	/// - [`set_params`](Self::set_params) has been called with the parameters
	///   `spectrum` was built from.
	fn transform(&mut self, spectrum: Spectrum) -> Spectrum;

	/// Rebuilds whatever the transform derives from the frequency grid.
	///
	/// Called when the grid changes, and once when the transform joins a chain.
	/// The default does nothing, which is right for a transform whose output
	/// depends only on the values it is handed.
	fn set_params(&mut self, _params: &Arc<SpectrumParams>) {}

	fn as_any_mut(&mut self) -> &mut dyn Any;
}

/// The ordered chain of transforms a spectrum passes through.
///
/// Position in the chain is the order of application. The [`TransformId`] rides
/// along so an entry can be matched with the config that describes it and the
/// control that edits it; it has no bearing on order.
///
/// The chain holds the frequency grid it was last given, so a transform added to
/// an established chain is handed the grid on arrival.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use melody_visualizer::spectrum::{
///     SpectrumBuffer, SpectrumParams, TransformChain, TransformId,
/// };
/// use melody_visualizer::spectrum::transforms::{VolumeNormalizer, volume_normalizer};
/// use melody_visualizer::traits::Configurable;
///
/// let params = Arc::new(SpectrumParams::exp_spaced(4, 200.0, 1600.0));
/// let spectrum = SpectrumBuffer::new(params.clone())
///     .fill(|values, _| values.copy_from_slice(&[1.0, 2.0, 4.0, 2.0]));
///
/// let mut chain = TransformChain::default();
/// chain.insert(
///     0,
///     TransformId(0),
///     Box::new(VolumeNormalizer::new(volume_normalizer::Config { rate: 0.1 })),
/// );
/// chain.set_params(&params);
///
/// // The normalizer scales the spectrum by its running peak.
/// assert_eq!(chain.apply(spectrum).values(), [0.25, 0.5, 1.0, 0.5]);
/// ```
#[derive(Debug, Default)]
pub struct TransformChain {
	entries: Vec<(TransformId, Box<dyn SpectrumTransform>)>,
	params: Option<Arc<SpectrumParams>>,
}

impl TransformChain {
	/// Runs `spectrum` through every transform, in chain order.
	pub fn apply(&mut self, spectrum: Spectrum) -> Spectrum {
		self.entries
			.iter_mut()
			.fold(spectrum, |spectrum, (_id, transform)| {
				transform.transform(spectrum)
			})
	}

	/// Hands `params` to every transform, if the frequency grid changed.
	///
	/// The grid is shared as an `Arc` and compared by pointer, which is how a
	/// parameter change reaches the DSP without an invalidation message
	/// (ARCHITECTURE.md § 3).
	pub fn set_params(&mut self, params: &Arc<SpectrumParams>) {
		if self
			.params
			.as_ref()
			.is_some_and(|current| Arc::ptr_eq(current, params))
		{
			return;
		}
		self.params = Some(params.clone());
		for (_id, transform) in self.entries.iter_mut() {
			transform.set_params(params);
		}
	}

	/// Adds `transform` at `index`, handing it the current frequency grid.
	///
	/// # Preconditions
	/// - `index <= len()`
	pub fn insert(
		&mut self,
		index: usize,
		id: TransformId,
		mut transform: Box<dyn SpectrumTransform>,
	) {
		if let Some(params) = &self.params {
			transform.set_params(params);
		}
		self.entries.insert(index, (id, transform));
	}

	/// Takes the transform identified by `id` out of the chain, closing the gap.
	pub fn remove(&mut self, id: TransformId) -> Option<Box<dyn SpectrumTransform>> {
		let index = self.index_of(id)?;
		Some(self.entries.remove(index).1)
	}

	/// Moves the transform at `from` to `to`, shifting the ones in between.
	///
	/// # Preconditions
	/// - `from` and `to` are both less than `len()`
	pub fn reorder(&mut self, from: usize, to: usize) {
		assert!(from < self.entries.len());
		assert!(to < self.entries.len());
		let entry = self.entries.remove(from);
		self.entries.insert(to, entry);
	}

	/// The transform identified by `id`, or `None` if the chain holds no such id.
	pub fn get_mut(&mut self, id: TransformId) -> Option<&mut dyn SpectrumTransform> {
		let index = self.index_of(id)?;
		Some(self.entries[index].1.as_mut())
	}

	/// The ids of the transforms, in chain order.
	pub fn ids(&self) -> impl Iterator<Item = TransformId> {
		self.entries.iter().map(|(id, _transform)| *id)
	}

	pub fn len(&self) -> usize {
		self.entries.len()
	}

	pub fn is_empty(&self) -> bool {
		self.entries.is_empty()
	}

	fn index_of(&self, id: TransformId) -> Option<usize> {
		self.entries
			.iter()
			.position(|(entry_id, _)| *entry_id == id)
	}
}

impl FromIterator<(TransformId, Box<dyn SpectrumTransform>)> for TransformChain {
	fn from_iter<Entries: IntoIterator<Item = (TransformId, Box<dyn SpectrumTransform>)>>(
		entries: Entries,
	) -> Self {
		TransformChain {
			entries: entries.into_iter().collect(),
			params: None,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	use std::sync::Mutex;

	/// What a [`Recorder`] was asked to do.
	#[derive(Debug, Clone, Copy, PartialEq, Eq)]
	enum Call {
		Transform(TransformId),
		SetParams(TransformId),
	}

	/// A transform that records its calls into a log shared with the test,
	/// leaving the spectrum untouched.
	#[derive(Debug)]
	struct Recorder {
		id: TransformId,
		log: Arc<Mutex<Vec<Call>>>,
	}

	impl SpectrumTransform for Recorder {
		fn transform(&mut self, spectrum: Spectrum) -> Spectrum {
			self.log.lock().unwrap().push(Call::Transform(self.id));
			spectrum
		}

		fn set_params(&mut self, _params: &Arc<SpectrumParams>) {
			self.log.lock().unwrap().push(Call::SetParams(self.id));
		}

		fn as_any_mut(&mut self) -> &mut dyn Any {
			self
		}
	}

	/// A chain of recorders with the given ids, and the log they share.
	fn recording_chain(ids: &[u64]) -> (TransformChain, Arc<Mutex<Vec<Call>>>) {
		let log = Arc::new(Mutex::new(Vec::new()));
		let chain = ids
			.iter()
			.map(|&id| {
				let transform = Recorder {
					id: TransformId(id),
					log: log.clone(),
				};
				(
					TransformId(id),
					Box::new(transform) as Box<dyn SpectrumTransform>,
				)
			})
			.collect::<TransformChain>();
		(chain, log)
	}

	fn test_params() -> Arc<SpectrumParams> {
		Arc::new(SpectrumParams::exp_spaced(8, 200.0, 20000.0))
	}

	#[test]
	fn transforms_run_in_chain_order() {
		let (mut chain, log) = recording_chain(&[7, 3, 5]);

		chain.apply(Spectrum::default());

		assert_eq!(
			*log.lock().unwrap(),
			[
				Call::Transform(TransformId(7)),
				Call::Transform(TransformId(3)),
				Call::Transform(TransformId(5)),
			],
			"position in the chain decides the order, not the id",
		);
	}

	#[test]
	fn set_params_reaches_every_transform_once_per_grid() {
		let (mut chain, log) = recording_chain(&[0, 1]);
		let params = test_params();

		chain.set_params(&params);
		// The same grid, handed over again, changes nothing.
		chain.set_params(&params.clone());
		chain.set_params(&params);

		assert_eq!(
			*log.lock().unwrap(),
			[
				Call::SetParams(TransformId(0)),
				Call::SetParams(TransformId(1))
			],
		);

		// A different grid is a different `Arc`, and reaches them again.
		chain.set_params(&test_params());
		assert_eq!(log.lock().unwrap().len(), 4);
	}

	#[test]
	fn a_transform_added_to_a_chain_is_handed_the_grid() {
		let (mut chain, log) = recording_chain(&[0]);
		chain.set_params(&test_params());
		log.lock().unwrap().clear();

		chain.insert(
			1,
			TransformId(1),
			Box::new(Recorder {
				id: TransformId(1),
				log: log.clone(),
			}),
		);

		assert_eq!(
			*log.lock().unwrap(),
			[Call::SetParams(TransformId(1))],
			"a transform joining an established chain needs the current grid",
		);
	}

	#[test]
	fn insert_remove_and_reorder_maintain_order() {
		let (mut chain, _log) = recording_chain(&[0, 1, 2]);

		chain.insert(
			1,
			TransformId(3),
			Box::new(Recorder {
				id: TransformId(3),
				log: Arc::new(Mutex::new(Vec::new())),
			}),
		);
		assert_eq!(
			chain.ids().collect::<Vec<_>>(),
			[0, 3, 1, 2].map(TransformId),
		);

		chain.reorder(0, 2);
		assert_eq!(
			chain.ids().collect::<Vec<_>>(),
			[3, 1, 0, 2].map(TransformId),
		);

		assert!(chain.remove(TransformId(1)).is_some());
		assert_eq!(chain.ids().collect::<Vec<_>>(), [3, 0, 2].map(TransformId));

		assert!(
			chain.remove(TransformId(1)).is_none(),
			"removing an id the chain does not hold leaves it alone",
		);
		assert_eq!(chain.len(), 3);
	}

	#[test]
	fn get_mut_addresses_a_transform_by_id() {
		let (mut chain, log) = recording_chain(&[4, 9]);

		chain
			.get_mut(TransformId(9))
			.expect("the chain holds id 9")
			.transform(Spectrum::default());

		assert_eq!(*log.lock().unwrap(), [Call::Transform(TransformId(9))]);
		assert!(chain.get_mut(TransformId(1)).is_none());
	}
}
