//! A real-time audio spectrum visualizer built on GTK 4 and JACK.
//!
//! The crate is a library so that its components can be exercised from `tests/`
//! and `benches/`; `src/main.rs` is a thin binary that wires them into a
//! `gtk::Application`. See `ARCHITECTURE.md` for the component and thread model.

pub mod app;
pub mod async_processor;
pub mod audio;
pub mod controllers;
pub mod error;
pub mod graphic;
pub mod gui;
pub mod note;
pub mod pubsub;
pub mod spectrum;
#[cfg(any(test, feature = "testing"))]
pub mod test_support;
pub mod traits;
