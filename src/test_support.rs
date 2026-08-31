//! Shared test-only helpers.
//!
//! Compiled for the crate's own tests, and for dependents that enable the
//! `testing` feature.

use futures::{channel::mpsc, prelude::*};
use glib::MainLoop;
use std::sync::Arc;

use crate::spectrum::{Spectrum, SpectrumBuffer, SpectrumParams};

/// The default frequency range, sampled at `samples` exponentially spaced bins.
///
/// # Preconditions
/// - `samples >= 2`
pub fn spectrum_params(samples: usize) -> Arc<SpectrumParams> {
	Arc::new(SpectrumParams::exp_spaced(samples, 200.0, 20000.0))
}

/// A spectrum holding `values`, on the grid [`spectrum_params`] builds for them.
///
/// # Preconditions
/// - `values.len() >= 2`
pub fn spectrum(values: &[f64]) -> Spectrum {
	SpectrumBuffer::new(spectrum_params(values.len()))
		.fill(|data, _params| data.copy_from_slice(values))
}

/// Run async test code inside a glib main loop.
///
/// Several components (the `PubSub` event bus, the JACK notification handler)
/// only deliver their effects when a glib main loop is pumping. This driver runs
/// such a loop and hands the test an async context that can *yield* control back
/// to it: awaiting `yield_rx.next()` resumes the test once every task higher than
/// `Priority::LOW` (e.g. a pending notification dispatch) has run. The loop quits
/// when the test future completes.
pub fn run_in_glib_main_loop<F, U>(f: F)
where
	F: FnOnce(mpsc::Receiver<()>) -> U + Send + 'static,
	U: Future<Output = ()>,
{
	let main_loop = Arc::new(MainLoop::new(None, false));
	let main_context = main_loop.context();

	// The idea is to have an asynchronous test case that can yield control back to
	// the main loop. We do that with a separate low-priority task that, every time
	// it is polled, wakes up the test code. In effect, every time the test code
	// yields, it is resumed when no tasks higher than PRIORITY_LOW are ready.
	let (mut yield_tx, yield_rx) = mpsc::channel(0);

	main_context.spawn_with_priority(glib::Priority::LOW, async move {
		loop {
			yield_tx.send(()).await.unwrap();
		}
	});

	let main_loop_clone = main_loop.clone();
	main_context.spawn(async move {
		let main_context = main_loop_clone.context();
		main_context.spawn_local(async move {
			// Quit main loop after test code completes.
			f(yield_rx).await;
			main_loop_clone.quit();
		});
	});

	main_loop.run();
}
