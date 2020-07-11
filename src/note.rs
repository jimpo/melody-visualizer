use std::cmp::{Ord, PartialOrd};
use std::convert::TryInto;
use std::ops::{Add, Sub};
use glib::bitflags::_core::cmp::Ordering;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PitchClass {
	Ab,
	A,
	Bb,
	B,
	C,
	Db,
	D,
	Eb,
	E,
	F,
	Gb,
	G,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
	pub octave: i8,
	pub pitch_class: PitchClass,
}

#[macro_export]
macro_rules! note {
	($pitch_class:ident, $octave:literal) => {
		$crate::note::Note {
			octave: $octave,
			pitch_class: $crate::note::PitchClass::$pitch_class,
		}
	}
}

impl Note {
	pub fn frequency(&self) -> f64 {
		self.log_frequency().exp2()
	}

	pub fn log_frequency(&self) -> f64 {
		const A4: Note = note!(A, 4);
		let x = (*self - A4) as f64;
		let y = x / 12.0;
		440.0f64.log2() + (*self - A4) as f64 / 12.0
	}

	fn to_half_step_count(&self) -> isize {
		let pitch_class_half_steps = match self.pitch_class {
			PitchClass::C  => 0,
			PitchClass::Db => 1,
			PitchClass::D  => 2,
			PitchClass::Eb => 3,
			PitchClass::E  => 4,
			PitchClass::F  => 5,
			PitchClass::Gb => 6,
			PitchClass::G  => 7,
			PitchClass::Ab => 8,
			PitchClass::A  => 9,
			PitchClass::Bb => 10,
			PitchClass::B  => 11,
		};
		self.octave as isize * 12 + pitch_class_half_steps
	}

	fn from_half_step_count(count: isize) -> Self {
		const PITCH_CLASSES: [PitchClass; 12] = [
			PitchClass::C,
			PitchClass::Db,
			PitchClass::D,
			PitchClass::Eb,
			PitchClass::E,
			PitchClass::F,
			PitchClass::Gb,
			PitchClass::G,
			PitchClass::Ab,
			PitchClass::A,
			PitchClass::Bb,
			PitchClass::B,
		];
		let octave = count.div_euclid(12)
			.try_into()
			.expect("from_half_step_count argument out of range");
		let pitch_class = PITCH_CLASSES[count.rem_euclid(12) as usize];
		Note {
			octave,
			pitch_class,
		}
	}
}

impl Ord for Note {
	fn cmp(&self, other: &Self) -> Ordering {
		self.to_half_step_count().cmp(&other.to_half_step_count())
	}
}

impl PartialOrd for Note {
	fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
		Some(self.cmp(other))
	}
}

impl Add<isize> for Note {
	type Output = Note;

	fn add(self, rhs: isize) -> Self::Output {
		Self::from_half_step_count(self.to_half_step_count() + rhs)
	}
}

impl Sub for Note {
	type Output = isize;

	fn sub(self, rhs: Self) -> Self::Output {
		self.to_half_step_count() - rhs.to_half_step_count()
	}
}

fn pitch_class_to_index(pitch_class: PitchClass) -> isize {
	match pitch_class {
		PitchClass::Ab => 0,
		PitchClass::A  => 1,
		PitchClass::Bb => 2,
		PitchClass::B  => 3,
		PitchClass::C  => 4,
		PitchClass::Db => 5,
		PitchClass::D  => 6,
		PitchClass::Eb => 7,
		PitchClass::E  => 8,
		PitchClass::F  => 9,
		PitchClass::Gb => 10,
		PitchClass::G  => 11,
	}
}

#[cfg(test)]
mod tests {
	#[test]
	fn frequencies() {
		assert_eq!(note!(A, 0).frequency(), 27.5);
		assert_eq!(note!(A, 4).frequency(), 440.0);
		assert!((note!(C, 8).frequency() - 4186.009).abs() < 0.001);
	}
}