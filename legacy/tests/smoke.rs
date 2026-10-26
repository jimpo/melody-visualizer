//! Proves the crate's public surface is reachable from an integration test.

use melody_visualizer::app::Config;
use melody_visualizer::note;
use melody_visualizer::spectrum::SpectrumParams;
use melody_visualizer::test_support::run_in_glib_main_loop;

#[test]
fn spectrum_params_span_the_configured_range() {
	let config = Config::default();
	let samples = config.samples_per_octave * (config.max_freq / config.min_freq).log2() as usize;
	let params = SpectrumParams::exp_spaced(samples, config.min_freq, config.max_freq);

	// The grid is built by exponentiating evenly spaced logs, so the endpoints
	// land within rounding distance of the configured bounds rather than on them.
	assert_eq!(params.samples(), samples);
	assert!((params.min_freq().unwrap() - config.min_freq).abs() < 1e-9);
	assert!((params.max_freq().unwrap() - config.max_freq).abs() < 1e-9);
}

#[test]
fn notes_are_ordered_by_pitch() {
	assert!(note!(A, 4) < note!(A, 5));
}

#[test]
fn test_support_fixtures_are_reachable() {
	run_in_glib_main_loop(|_yield_rx| async {});
}
