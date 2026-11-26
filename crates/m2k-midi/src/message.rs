//! Wire-level MIDI message types. This file is the shared contract between the MIDI layer,
//! the engine and the UI (TypeScript bindings are generated from it).
//!
//! Conventions (see AGENTS.md): channels are **1..=16**; notes/controllers/7-bit values are
//! `0..=127`; 14-bit values are `0..=16383`; pitch bend is `0..=16383` with `8192` as center.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One complete, decoded MIDI message.
///
/// `NoteOn` with velocity 0 is kept as it appeared on the wire (the monitor shows what the device
/// really sent). Use [`MidiMessage::note_event`] to get the normalized on/off meaning.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MidiMessage {
    NoteOff { channel: u8, note: u8, velocity: u8 },
    NoteOn { channel: u8, note: u8, velocity: u8 },
    /// Polyphonic key pressure (per-note aftertouch).
    PolyPressure { channel: u8, note: u8, pressure: u8 },
    ControlChange { channel: u8, controller: u8, value: u8 },
    ProgramChange { channel: u8, program: u8 },
    /// Channel pressure (mono aftertouch).
    ChannelPressure { channel: u8, pressure: u8 },
    /// 14-bit pitch bend, `0..=16383`, center `8192`.
    PitchBend { channel: u8, value: u16 },
    /// A complete System Exclusive message, including the leading `F0` and trailing `F7`.
    SysEx { data: Vec<u8> },
    /// MIDI Time Code quarter frame (`F1`). `piece` is the message type `0..=7`, `value` the nibble.
    MtcQuarterFrame { piece: u8, value: u8 },
    /// Song Position Pointer (`F2`), in MIDI beats (sixteenth notes), `0..=16383`.
    SongPosition { position: u16 },
    SongSelect { song: u8 },
    TuneRequest,
    /// Timing clock (`F8`), 24 per quarter note.
    Clock,
    Start,
    Continue,
    Stop,
    ActiveSensing,
    Reset,
    /// Anything that could not be decoded (undefined status bytes, truncated messages).
    Unknown { data: Vec<u8> },
}

/// Normalized meaning of a note message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteEvent {
    pub channel: u8,
    pub note: u8,
    /// `true` for a real Note On (velocity > 0).
    pub on: bool,
    /// Note-on velocity `1..=127`, or the release velocity for a note off (`0` when the device
    /// sent Note On with velocity 0).
    pub velocity: u8,
}

impl MidiMessage {
    /// The channel (1..=16) for channel voice messages, `None` for system messages.
    pub fn channel(&self) -> Option<u8> {
        match self {
            MidiMessage::NoteOff { channel, .. }
            | MidiMessage::NoteOn { channel, .. }
            | MidiMessage::PolyPressure { channel, .. }
            | MidiMessage::ControlChange { channel, .. }
            | MidiMessage::ProgramChange { channel, .. }
            | MidiMessage::ChannelPressure { channel, .. }
            | MidiMessage::PitchBend { channel, .. } => Some(*channel),
            _ => None,
        }
    }

    /// Normalizes Note On / Note Off, treating Note On with velocity 0 as a note off.
    pub fn note_event(&self) -> Option<NoteEvent> {
        match *self {
            MidiMessage::NoteOn { channel, note, velocity } => {
                Some(NoteEvent { channel, note, on: velocity > 0, velocity })
            }
            MidiMessage::NoteOff { channel, note, velocity } => {
                Some(NoteEvent { channel, note, on: false, velocity })
            }
            _ => None,
        }
    }

    /// System real-time messages (clock, start, stop, ...) that may be interleaved anywhere.
    pub fn is_realtime(&self) -> bool {
        matches!(
            self,
            MidiMessage::Clock
                | MidiMessage::Start
                | MidiMessage::Continue
                | MidiMessage::Stop
                | MidiMessage::ActiveSensing
                | MidiMessage::Reset
        )
    }

    /// High-rate housekeeping messages that monitors usually hide by default.
    pub fn is_noise(&self) -> bool {
        matches!(self, MidiMessage::Clock | MidiMessage::ActiveSensing)
    }
}

/// Which registered/non-registered parameter space a parameter number belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ParameterKind {
    /// Non-Registered Parameter Number (CC 99 / CC 98).
    Nrpn,
    /// Registered Parameter Number (CC 101 / CC 100).
    Rpn,
}

/// What caused a [`ParameterEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ParameterChange {
    /// Data Entry MSB (CC 6). The LSB is reset to 0 as the MIDI spec prescribes.
    DataMsb,
    /// Data Entry LSB (CC 38). Together with the last MSB this completes a 14-bit value.
    DataLsb,
    /// Data Increment (CC 96). `amount` is the CC value (usually ignored / treated as 1).
    Increment,
    /// Data Decrement (CC 97).
    Decrement,
}

/// An NRPN / RPN data change, assembled from the underlying control changes by `MidiState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
pub struct ParameterEvent {
    pub channel: u8,
    pub kind: ParameterKind,
    /// 14-bit parameter number `0..=16383` (`msb << 7 | lsb`).
    pub number: u16,
    pub change: ParameterChange,
    /// Current data entry MSB for this parameter (after applying this change).
    pub msb: u8,
    /// Current data entry LSB for this parameter (after applying this change).
    pub lsb: u8,
    /// Raw value of the CC that caused this event (the increment/decrement amount for those).
    pub amount: u8,
}

impl ParameterEvent {
    /// The value as a 7-bit quantity (data entry MSB only).
    pub fn value7(&self) -> u8 {
        self.msb
    }

    /// The value as a 14-bit quantity (`msb << 7 | lsb`).
    pub fn value14(&self) -> u16 {
        ((self.msb as u16) << 7) | self.lsb as u16
    }
}

/// A completed 14-bit control change: MSB controller `0..=31` paired with LSB controller
/// `msb + 32`. Emitted by `MidiState` when the LSB arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
pub struct ControlChange14 {
    pub channel: u8,
    /// The MSB controller number `0..=31`.
    pub controller: u8,
    /// `0..=16383`.
    pub value: u16,
}

/// High-level events derived from the raw message stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Derived {
    Parameter(ParameterEvent),
    ControlChange14(ControlChange14),
}

/// Helpers for presenting MIDI data to humans.
pub mod names {
    const NOTE_NAMES: [&str; 12] =
        ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

    /// Note name such as `C4`. `middle_c_octave` is the octave number assigned to note 60
    /// (4 in scientific pitch notation, 3 in the Yamaha convention).
    pub fn note_name(note: u8, middle_c_octave: i8) -> String {
        let octave = (note as i16 / 12) - 5 + middle_c_octave as i16;
        format!("{}{}", NOTE_NAMES[(note % 12) as usize], octave)
    }
}
