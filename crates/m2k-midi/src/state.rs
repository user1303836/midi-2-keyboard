//! Per-port MIDI state tracking.
//!
//! CONTRACT (signatures are relied upon by the engine; keep them stable):
//!
//! [`MidiState`] remembers, per channel: which notes are held (and their velocity), the last
//! value of every controller, pitch bend, channel/poly pressure, program, and the currently
//! selected NRPN/RPN. [`MidiState::update`] also returns the high-level [`Derived`] events a
//! message completes:
//!
//! - NRPN/RPN: CC 99/98 select an NRPN, CC 101/100 select an RPN (selecting one deselects the
//!   other; RPN 127/127 is "null" and deselects). CC 6 / 38 / 96 / 97 then produce a
//!   [`Derived::Parameter`] for the selected parameter. A data entry MSB resets the LSB to 0.
//!   Data entry with nothing selected produces nothing.
//! - 14-bit CC: when a controller `32..=63` (LSB) arrives and its MSB partner (`n - 32`) has been
//!   seen on that channel, emit [`Derived::ControlChange14`] with `msb << 7 | lsb`.
//!   (An MSB resets the stored LSB to 0 but does not emit by itself.)
//!
//! Channels are 1..=16 in the public API.

use crate::message::{Derived, MidiMessage};

/// Up to two derived events for one message, without allocating.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DerivedEvents {
    items: [Option<Derived>; 2],
}

impl DerivedEvents {
    pub fn push(&mut self, event: Derived) {
        for slot in &mut self.items {
            if slot.is_none() {
                *slot = Some(event);
                return;
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = Derived> + '_ {
        self.items.iter().flatten().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.items[0].is_none()
    }
}

/// Tracks the state of one MIDI input port.
#[derive(Debug, Clone, Default)]
pub struct MidiState {}

impl MidiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a message and returns the derived high-level events it completes.
    pub fn update(&mut self, message: &MidiMessage) -> DerivedEvents {
        let _ = message;
        todo!("m2k-midi: MidiState::update")
    }

    /// Velocity of a held note, `0` when the note is not held.
    pub fn note_velocity(&self, channel: u8, note: u8) -> u8 {
        let _ = (channel, note);
        todo!("m2k-midi: MidiState::note_velocity")
    }

    pub fn is_note_held(&self, channel: u8, note: u8) -> bool {
        self.note_velocity(channel, note) > 0
    }

    /// All currently held notes as `(channel, note, velocity)`.
    pub fn held_notes(&self) -> Vec<(u8, u8, u8)> {
        todo!("m2k-midi: MidiState::held_notes")
    }

    /// Last received value of a controller, `None` if never seen.
    pub fn control(&self, channel: u8, controller: u8) -> Option<u8> {
        let _ = (channel, controller);
        todo!("m2k-midi: MidiState::control")
    }

    /// Last pitch bend value (`8192` if never seen).
    pub fn pitch_bend(&self, channel: u8) -> u16 {
        let _ = channel;
        todo!("m2k-midi: MidiState::pitch_bend")
    }

    pub fn channel_pressure(&self, channel: u8) -> u8 {
        let _ = channel;
        todo!("m2k-midi: MidiState::channel_pressure")
    }

    pub fn poly_pressure(&self, channel: u8, note: u8) -> u8 {
        let _ = (channel, note);
        todo!("m2k-midi: MidiState::poly_pressure")
    }

    /// Last received program, `None` if never seen.
    pub fn program(&self, channel: u8) -> Option<u8> {
        let _ = channel;
        todo!("m2k-midi: MidiState::program")
    }

    /// Forgets everything (use after a device disconnects or on MIDI reset).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}
