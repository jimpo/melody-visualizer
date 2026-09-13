//! The DSP chain end to end: a known chord in, a spectrum out.
//!
//! The unit tests pin each stage in isolation, which is what makes a failure
//! easy to place. This one pins their composition — it is what fails when two
//! correct stages are wired together wrongly, or when a change to the
//! value-range contract quietly breaks the chain's output range.
//!
//! Everything here is arithmetic over a synthesized signal: no JACK, no thread,
//! no clock, and the same result on every run.

use std::sync::Arc;

use melody_visualizer::app::Config;
use melody_visualizer::app::config::{SpectrumGeneratorConfig, SpectrumTransformConfig};
use melody_visualizer::spectrum::generators::audio;
use melody_visualizer::spectrum::{Spectrum, SpectrumBuffer, TransformId};
use melody_visualizer::test_support::{renderer, sample_reader, sine_wave};

const SAMPLE_RATE: u32 = 48_000;

/// A peak counts as one when it stands this far above the normalized output.
/// The quietest note of the chord reaches about 0.27, and the leakage between
/// the notes stays below 0.01.
const PEAK_THRESHOLD: f64 = 0.05;

/// The id of the first stage of the default chain whose config `is_kind`
/// accepts.
///
/// Found by kind rather than by position, since the stages ahead of it in
/// `Config::default` change.
fn stage_id(config: &Config, is_kind: fn(&SpectrumTransformConfig) -> bool) -> TransformId {
	config
		.spectrum_transforms
		.iter()
		.find(|entry| is_kind(&entry.config))
		.expect("the default chain holds the stage")
		.id
}

/// The index of the largest value.
fn loudest_index(spectrum: &Spectrum) -> usize {
	let values = spectrum.values();
	(0..values.len())
		.max_by(|&a, &b| values[a].total_cmp(&values[b]))
		.expect("a spectrum has bins")
}

/// The indices of the local maxima above [`PEAK_THRESHOLD`].
fn peak_indices(spectrum: &Spectrum) -> Vec<usize> {
	let values = spectrum.values();
	(1..values.len() - 1)
		.filter(|&index| {
			values[index] > PEAK_THRESHOLD
				&& values[index] >= values[index - 1]
				&& values[index] > values[index + 1]
		})
		.collect()
}

/// The amplitude of the band within a quarter-octave of `frequency`.
///
/// The stages ahead of it spread a note over neighbouring bins by differing
/// amounts, so a band is what the note's magnitude means; the peak value alone
/// is not it. The bins hold amplitude and the powers of a band's bins add, so
/// the band's amplitude is the root of the sum of their squares.
fn band_amplitude(spectrum: &Spectrum, frequency: f64) -> f64 {
	let quarter_octave = 2.0f64.powf(0.25);
	std::iter::zip(spectrum.params().frequencies(), spectrum.values())
		.filter(|(bin, _)| **bin > frequency / quarter_octave && **bin < frequency * quarter_octave)
		.map(|(_, amplitude)| amplitude * amplitude)
		.sum::<f64>()
		.sqrt()
}

#[test]
fn a_chord_through_the_default_chain_comes_out_as_three_notes() {
	// Notes an octave apart are a harmonic series of the lowest, which the
	// harmonic summation folds into it. Pinned with it bypassed, so the chord
	// tests the rest of the chain.
	let mut config = Config::default();
	let harmonic_summation = stage_id(&config, |config| {
		matches!(config, SpectrumTransformConfig::HarmonicSummation(_))
	});
	config
		.set_transform_enabled(harmonic_summation, false)
		.unwrap();
	let SpectrumGeneratorConfig::Audio(generator_config) = &config.spectrum_generator;
	let window = audio::window_samples(generator_config.window_ms, SAMPLE_RATE);

	// Three notes an octave apart, each at half the amplitude of the one below.
	// They sit on exact DFT bin centres, so the analysis resolves them without
	// the leakage a frequency between two bins would produce.
	let dft_bin = SAMPLE_RATE as f64 / window as f64;
	let chord = [
		(76.0 * dft_bin, 1.0),
		(152.0 * dft_bin, 0.5),
		(304.0 * dft_bin, 0.25),
	];

	let signal = sine_wave(&chord, SAMPLE_RATE, window);
	let params = Arc::new(config.spectrum_params());
	let spectrum = renderer(&config, sample_reader(&signal), SAMPLE_RATE)
		.render(SpectrumBuffer::new(params.clone()));

	assert!(
		spectrum.values().iter().all(|value| value.is_finite()),
		"the chain produced a NaN or an infinity",
	);

	// Three notes in, three peaks out.
	let peaks = peak_indices(&spectrum);
	assert_eq!(
		peaks.len(),
		3,
		"expected one peak per note, found {:?}",
		peaks
			.iter()
			.map(|&index| params.frequencies()[index])
			.collect::<Vec<_>>(),
	);

	// Each peak sits on the note that produced it.
	let bin_width = params.log_frequencies()[1] - params.log_frequencies()[0];
	for (&peak, (frequency, _amplitude)) in std::iter::zip(&peaks, chord) {
		let error = params.log_frequencies()[peak] - frequency.log2();
		assert!(
			error.abs() <= bin_width,
			"the peak for {frequency} Hz is {} bins away",
			error / bin_width,
		);
	}

	// Equal spacing per octave is the property the whole log-frequency design
	// exists to provide, and a fault in the binning maths shows here first.
	assert_eq!(
		peaks[1] - peaks[0],
		config.samples_per_octave,
		"notes an octave apart sit one octave of bins apart",
	);
	assert_eq!(peaks[2] - peaks[1], peaks[1] - peaks[0]);

	// A spectrum holds amplitude, so halving an amplitude halves a note's share.
	// Pinned with the diffuser bypassed: it spreads amplitude linearly, which
	// takes more of the power of a narrow treble peak than of a wide bass hump,
	// and so moves these ratios by up to a fifth.
	let diffuser = stage_id(&config, |config| {
		matches!(config, SpectrumTransformConfig::Diffuser(_))
	});
	let mut undiffused = config.clone();
	undiffused.set_transform_enabled(diffuser, false).unwrap();
	let undiffused = renderer(&undiffused, sample_reader(&signal), SAMPLE_RATE)
		.render(SpectrumBuffer::new(params.clone()));

	let amplitude = chord.map(|(frequency, _)| band_amplitude(&undiffused, frequency));
	for (note, expected_ratio) in [(1, 0.5), (2, 0.25)] {
		let ratio = amplitude[note] / amplitude[0];
		assert!(
			(ratio / expected_ratio - 1.0).abs() < 0.05,
			"note {note} holds {ratio} of the lowest note's amplitude, expected {expected_ratio}",
		);
	}

	// The chain ends in the volume normalizer, whose output is a fraction of a
	// running peak that the first spectrum seeds — so the loudest bin is 1.0.
	let loudest = spectrum.values().iter().cloned().fold(0.0f64, f64::max);
	assert!(
		(loudest - 1.0).abs() < 1e-12,
		"the chain's output should peak at 1.0, peaked at {loudest}",
	);
	assert!(
		spectrum.values().iter().all(|&value| value >= 0.0),
		"amplitude is never negative, and the consumer clamps only from above",
	);
}

#[test]
fn a_silent_input_produces_a_silent_spectrum() {
	let config = Config::default();
	let params = Arc::new(config.spectrum_params());
	let mut renderer = renderer(&config, sample_reader(&[0.0; 2048]), SAMPLE_RATE);

	let spectrum = renderer.render(SpectrumBuffer::new(params));

	assert!(
		spectrum.values().iter().all(|&value| value == 0.0),
		"silence in, silence out — and nothing the normalizer divides by zero",
	);
}

#[test]
fn a_switched_off_stage_is_the_same_as_no_stage_at_all() {
	let config = Config::default();
	let SpectrumGeneratorConfig::Audio(generator_config) = &config.spectrum_generator;
	let window = audio::window_samples(generator_config.window_ms, SAMPLE_RATE);
	let signal = sine_wave(&[(440.0, 1.0)], SAMPLE_RATE, window);
	let params = Arc::new(config.spectrum_params());

	// The diffuser spreads a partial across neighbouring bins, so leaving it out
	// is a difference the output shows.
	let diffuser = stage_id(&config, |config| {
		matches!(config, SpectrumTransformConfig::Diffuser(_))
	});

	let mut bypassed = config.clone();
	bypassed.set_transform_enabled(diffuser, false).unwrap();

	let mut removed = config.clone();
	removed
		.spectrum_transforms
		.retain(|entry| entry.id != diffuser);

	let render = |config| {
		renderer(config, sample_reader(&signal), SAMPLE_RATE)
			.render(SpectrumBuffer::new(params.clone()))
	};

	assert_eq!(render(&bypassed).values(), render(&removed).values());
	assert_ne!(render(&bypassed).values(), render(&config).values());
}

#[test]
fn a_note_whose_second_harmonic_is_loudest_is_brightest_at_its_fundamental() {
	let config = Config::default();
	let SpectrumGeneratorConfig::Audio(generator_config) = &config.spectrum_generator;
	let window = audio::window_samples(generator_config.window_ms, SAMPLE_RATE);

	// A brass-like note: the second harmonic carries twice the fundamental's
	// amplitude. Every partial sits on a DFT bin centre, as in the chord above.
	let dft_bin = SAMPLE_RATE as f64 / window as f64;
	let fundamental = 22.0 * dft_bin;
	let note = [
		(fundamental, 0.5),
		(2.0 * fundamental, 1.0),
		(3.0 * fundamental, 0.6),
		(4.0 * fundamental, 0.4),
	];
	let signal = sine_wave(&note, SAMPLE_RATE, window);
	let params = Arc::new(config.spectrum_params());
	let render = |config| {
		renderer(config, sample_reader(&signal), SAMPLE_RATE)
			.render(SpectrumBuffer::new(params.clone()))
	};
	let bin_width = params.log_frequencies()[1] - params.log_frequencies()[0];
	let bins_from = |index: usize, frequency: f64| {
		((params.log_frequencies()[index] - frequency.log2()) / bin_width).abs()
	};

	let summed = render(&config);
	let loudest = loudest_index(&summed);
	assert!(
		bins_from(loudest, fundamental) <= 1.0,
		"the brightest bin is {} Hz, expected the fundamental at {fundamental} Hz",
		params.frequencies()[loudest],
	);

	// Without the stage the spiral lights the second harmonic instead.
	let mut unsummed = config.clone();
	let harmonic_summation = stage_id(&config, |config| {
		matches!(config, SpectrumTransformConfig::HarmonicSummation(_))
	});
	unsummed
		.set_transform_enabled(harmonic_summation, false)
		.unwrap();
	let loudest = loudest_index(&render(&unsummed));
	assert!(bins_from(loudest, 2.0 * fundamental) <= 1.0);
}
