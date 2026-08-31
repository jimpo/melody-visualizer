use std::cmp::Ordering;
use std::cmp::{Ord, PartialOrd};
use std::convert::TryInto;
use std::ops::{Add, RangeBounds, Sub};

#[derive(Debug, derive_more::Display, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, derive_more::Display, Clone, Copy, PartialEq, Eq)]
#[display("{}{}", pitch_class, octave)]
pub struct Note {
	pub octave: i8,
	pub pitch_class: PitchClass,
}

#[macro_export]
macro_rules! note {
	($pitch_class:ident, $octave:expr_2021) => {
		$crate::note::Note {
			octave: $octave,
			pitch_class: $crate::note::PitchClass::$pitch_class,
		}
	};
}

impl Note {
	pub fn frequency(&self) -> f64 {
		self.log_frequency().exp2()
	}

	pub fn log_frequency(&self) -> f64 {
		const A4: Note = note!(A, 4);
		440.0f64.log2() + (*self - A4) as f64 / 12.0
	}

	// TODO: Implement iter::Step when that trait is stable.
	pub fn next(&self) -> Self {
		match self.pitch_class {
			PitchClass::C => note!(Db, self.octave),
			PitchClass::Db => note!(D, self.octave),
			PitchClass::D => note!(Eb, self.octave),
			PitchClass::Eb => note!(E, self.octave),
			PitchClass::E => note!(F, self.octave),
			PitchClass::F => note!(Gb, self.octave),
			PitchClass::Gb => note!(G, self.octave),
			PitchClass::G => note!(Ab, self.octave),
			PitchClass::Ab => note!(A, self.octave),
			PitchClass::A => note!(Bb, self.octave),
			PitchClass::Bb => note!(B, self.octave),
			PitchClass::B => note!(C, self.octave + 1),
		}
	}

	fn to_half_step_count(&self) -> isize {
		let pitch_class_half_steps = match self.pitch_class {
			PitchClass::C => 0,
			PitchClass::Db => 1,
			PitchClass::D => 2,
			PitchClass::Eb => 3,
			PitchClass::E => 4,
			PitchClass::F => 5,
			PitchClass::Gb => 6,
			PitchClass::G => 7,
			PitchClass::Ab => 8,
			PitchClass::A => 9,
			PitchClass::Bb => 10,
			PitchClass::B => 11,
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
		let octave = count
			.div_euclid(12)
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

pub struct NoteIterator {
	next: Option<Note>,
	end: Note,
}

impl Iterator for NoteIterator {
	type Item = Note;

	fn next(&mut self) -> Option<Self::Item> {
		let next = self.next?;
		if next > self.end {
			self.next = None;
			return None;
		}
		self.next = Some(next.next());
		Some(next)
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		(0, None)
	}
}

pub fn iter(range: impl RangeBounds<Note>) -> NoteIterator {
	use std::ops::Bound;
	let next = match range.start_bound() {
		Bound::Included(&note) => Some(note),
		Bound::Excluded(&note) => Some(note.next()),
		Bound::Unbounded => None,
	};
	let end = match range.end_bound() {
		Bound::Included(&note) => note,
		Bound::Excluded(&note) => note + -1,
		Bound::Unbounded => note!(B, i8::MAX),
	};
	NoteIterator { next, end }
}

#[cfg(test)]
mod tests {
	use super::iter;

	#[test]
	fn frequencies() {
		assert!((note!(A, 0).frequency() - 27.5).abs() < 0.001);
		assert!((note!(A, 4).frequency() - 440.0).abs() < 0.001);
		assert!((note!(C, 8).frequency() - 4186.009).abs() < 0.001);
	}

	#[test]
	fn note_iterator() {
		for note in iter(note!(A, 2)..note!(B, 3)) {
			println!("{}", &note);
		}
	}
}
