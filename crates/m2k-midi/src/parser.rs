//! Bytes -> [`MidiMessage`].
//!
//! CONTRACT (signatures are relied upon by the engine and the CLI; keep them stable):
//!
//! - [`parse_message`] decodes exactly one complete message as delivered by OS MIDI APIs.
//! - [`Parser`] is a streaming parser for arbitrary byte chunks: running status, real-time bytes
//!   interleaved inside other messages (including inside SysEx), SysEx split across chunks,
//!   SysEx terminated by a new status byte instead of `F7`, stray data bytes.
//! - [`encode`] is the inverse of [`parse_message`].

use crate::message::MidiMessage;

/// Decodes one complete MIDI message. Never panics; undecodable input becomes
/// [`MidiMessage::Unknown`].
pub fn parse_message(bytes: &[u8]) -> MidiMessage {
    let _ = bytes;
    todo!("m2k-midi: parse_message")
}

/// Encodes a message back to wire bytes.
pub fn encode(message: &MidiMessage) -> Vec<u8> {
    let _ = message;
    todo!("m2k-midi: encode")
}

/// Streaming parser. Feed it arbitrary chunks of a MIDI byte stream.
#[derive(Debug, Default)]
pub struct Parser {}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds bytes, invoking `emit` for every completed message, in stream order.
    pub fn feed(&mut self, bytes: &[u8], emit: &mut dyn FnMut(MidiMessage)) {
        let _ = (bytes, emit);
        todo!("m2k-midi: Parser::feed")
    }

    /// Drops any partially received message and forgets running status.
    pub fn reset(&mut self) {
        todo!("m2k-midi: Parser::reset")
    }
}
