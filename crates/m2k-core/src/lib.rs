//! Core of midi-2-keyboard: the config model, its persistence, and the engine.

pub mod engine;
pub mod events;
pub mod model;
pub mod store;

pub use engine::{Engine, EngineOptions};
pub use events::{EngineEvent, EngineStatus, FiredLog, MidiLog, Outcome, SkipReason};
pub use model::*;
