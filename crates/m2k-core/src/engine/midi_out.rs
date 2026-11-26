//! MIDI output: the [`MidiSink`] the engine sends through (the "Send MIDI" action and macro
//! step, feedback lights) and the compiled form of a [`MidiSend`].

use std::sync::Arc;

use m2k_midi::names::note_name;
use m2k_midi::output::{InvalidMidi, split_messages};

use super::text::shorten;
use crate::model::{MidiSend, SendValue};

/// Where the engine sends MIDI. Owned by the engine thread like the output backend, and just as
/// strict: it must never block (an output that is slow or gone fails instead of waiting).
pub trait MidiSink: Send {
    /// Sends one complete MIDI message (a channel or system message, or a whole SysEx from `F0`
    /// to `F7`) to the output port named `port`.
    fn send(&self, port: &str, bytes: &[u8]) -> Result<(), String>;

    /// Whether an output port with this name exists (it may only be opened on first use).
    fn has_port(&self, name: &str) -> bool;

    /// The output that answers the input port called `input` (a "Send MIDI" without a port
    /// goes back to the device the message came from). By default the output of the same name;
    /// the app's sink also pairs the extra ports Windows names `MIDIINn (…)` / `MIDIOUTn (…)`.
    fn paired_output(&self, input: &str) -> Option<String> {
        self.has_port(input).then(|| input.to_string())
    }
}

/// No MIDI outputs at all (the engine's default).
pub struct NullMidiSink;

impl MidiSink for NullMidiSink {
    fn send(&self, _port: &str, _bytes: &[u8]) -> Result<(), String> {
        Err("no MIDI output available".into())
    }

    fn has_port(&self, _name: &str) -> bool {
        false
    }
}

/// A [`MidiSend`] ready to go out.
#[derive(Debug, Clone)]
pub enum CSend {
    /// A channel message; `value` says which data byte a [`SendValue`] fills.
    Channel { bytes: [u8; 3], len: usize, value: Option<(usize, SendValue)> },
    /// Fixed bytes, as complete messages.
    Raw(Arc<[Box<[u8]>]>),
}

impl CSend {
    /// Compiles `send`. Raw bytes that are not MIDI fail with a message naming the token at
    /// fault.
    pub fn new(send: &MidiSend) -> Result<CSend, String> {
        let status = |kind: u8, channel: u8| kind | (channel.clamp(1, 16) - 1);
        Ok(match send {
            MidiSend::NoteOn { channel, note, velocity } => CSend::Channel {
                bytes: [status(0x90, *channel), note & 0x7F, 0],
                len: 3,
                value: Some((2, *velocity)),
            },
            // Release velocity 64: what the MIDI spec asks of senders without release velocity.
            MidiSend::NoteOff { channel, note } => CSend::Channel {
                bytes: [status(0x80, *channel), note & 0x7F, 64],
                len: 3,
                value: None,
            },
            MidiSend::ControlChange { channel, controller, value } => CSend::Channel {
                bytes: [status(0xB0, *channel), controller & 0x7F, 0],
                len: 3,
                value: Some((2, *value)),
            },
            MidiSend::ProgramChange { channel, program } => CSend::Channel {
                bytes: [status(0xC0, *channel), 0, 0],
                len: 2,
                value: Some((1, *program)),
            },
            MidiSend::Raw { bytes } => CSend::Raw(parse_raw(bytes)?.into()),
        })
    }

    /// Sends every message to `port`; `incoming` (0..=127) stands for [`SendValue::Incoming`].
    /// Stops at the first failure.
    pub fn send(&self, sink: &dyn MidiSink, port: &str, incoming: u8) -> Result<(), String> {
        match self {
            CSend::Channel { bytes, len, value } => {
                sink.send(port, &filled(*bytes, *value, incoming)[..*len])
            }
            CSend::Raw(messages) => {
                messages.iter().try_for_each(|message| sink.send(port, message))
            }
        }
    }

    /// The message for the monitor, worded like the editor does: `CC 20 = 127 (ch 1)`,
    /// `note C3 at 100 (ch 10)`, `note C3 off (ch 1)`, `program 5 (ch 1)`, `F0 7F 7F 06 02 F7`.
    pub fn text(&self, incoming: u8, middle_c_octave: i8) -> String {
        match self {
            CSend::Channel { bytes, value, .. } => {
                let bytes = filled(*bytes, *value, incoming);
                let channel = (bytes[0] & 0x0F) + 1;
                let note = |n: u8| note_name(n, middle_c_octave);
                match bytes[0] & 0xF0 {
                    0x90 if bytes[2] == 0 => format!("note {} off (ch {channel})", note(bytes[1])),
                    0x90 => format!("note {} at {} (ch {channel})", note(bytes[1]), bytes[2]),
                    0x80 => format!("note {} off (ch {channel})", note(bytes[1])),
                    0xB0 => format!("CC {} = {} (ch {channel})", bytes[1], bytes[2]),
                    _ => format!("program {} (ch {channel})", bytes[1]),
                }
            }
            CSend::Raw(messages) => {
                let hex: Vec<String> =
                    messages.iter().flat_map(|m| m.iter()).map(|b| format!("{b:02X}")).collect();
                shorten(&hex.join(" "), 40).into_owned()
            }
        }
    }
}

fn filled(mut bytes: [u8; 3], value: Option<(usize, SendValue)>, incoming: u8) -> [u8; 3] {
    if let Some((at, value)) = value {
        bytes[at] = match value {
            SendValue::Fixed { value } => value & 0x7F,
            SendValue::Incoming => incoming & 0x7F,
        };
    }
    bytes
}

/// Space separated hex bytes (`F0 7F 7F 06 02 F7`, `90 3C 7F 3E 7F`) as complete MIDI messages,
/// cut by [`split_messages`] (the rules the output itself applies, so a config that validates
/// here is never refused when sent). Blank text is no message. The error names the byte at fault
/// as it was written.
pub fn parse_raw(text: &str) -> Result<Vec<Box<[u8]>>, String> {
    let mut tokens: Vec<&str> = Vec::new();
    let mut bytes: Vec<u8> = Vec::new();
    for token in text.split_whitespace() {
        let digits = token.strip_prefix("0x").or_else(|| token.strip_prefix("0X")).unwrap_or(token);
        let byte = ((1..=2).contains(&digits.len())
            && digits.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| u8::from_str_radix(digits, 16).ok())
        .flatten()
        .ok_or_else(|| format!("\"{token}\" is not a hex byte (00 to FF)"))?;
        tokens.push(token);
        bytes.push(byte);
    }
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    // Positions as the user counts them (1 = the first byte), with the byte as written.
    let at = |offset: usize| match tokens.get(offset) {
        Some(token) => format!("byte {} (\"{token}\")", offset + 1),
        None => "the end".to_string(),
    };
    match split_messages(&bytes) {
        Ok(messages) => Ok(messages.into_iter().map(Vec::into_boxed_slice).collect()),
        Err(InvalidMidi::MissingStatus { offset, .. }) => {
            Err(format!("{} is a data byte where a status byte (80 to FF) has to come", at(offset)))
        }
        Err(InvalidMidi::UndefinedStatus { offset, .. }) => {
            Err(format!("{} is not a defined MIDI status byte", at(offset)))
        }
        Err(InvalidMidi::Incomplete { start, offset }) => Err(format!(
            "the message starting at {} is incomplete: it needs a data byte (00 to 7F) at {}",
            at(start),
            at(offset)
        )),
        Err(InvalidMidi::UnterminatedSysEx { start }) => {
            Err(format!("the SysEx starting at {} has no F7 at its end", at(start)))
        }
        Err(InvalidMidi::InterruptedSysEx { start, offset }) => Err(format!(
            "the SysEx starting at {} is interrupted by {} (only 00 to 7F may come before F7)",
            at(start),
            at(offset)
        )),
        Err(InvalidMidi::StrayEndOfSysEx { offset }) => {
            Err(format!("{} ends a SysEx that never started (F0)", at(offset)))
        }
        Err(other) => Err(other.to_string()),
    }
}
