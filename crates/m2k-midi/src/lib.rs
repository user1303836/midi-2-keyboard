//! MIDI domain for midi-2-keyboard.
//!
//! - [`message`]: wire-level message types (the shared contract; generates TS bindings).
//! - [`parser`]: bytes -> [`MidiMessage`] (running status, interleaved real-time, SysEx assembly).
//! - [`state`]: per-port state tracking (held notes, CC values, NRPN/RPN, 14-bit CC pairs).
//! - [`input`]: device enumeration, connection management and hot-plug handling on top of `midir`.
//! - [`output`]: the same for outputs: lazily opened ports and a send queue that never blocks.

pub mod input;
pub mod message;
pub mod output;
pub mod parser;
pub mod state;

pub use message::{
    ControlChange14, Derived, MidiMessage, NoteEvent, ParameterChange, ParameterEvent,
    ParameterKind, names,
};
pub use parser::{Parser, encode, parse_message};
pub use state::{DerivedEvents, MidiState};
