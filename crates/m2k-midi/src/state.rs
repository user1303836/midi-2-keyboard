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
//!
//! Details beyond the contract:
//!
//! - CC 38 (data entry LSB) is also the LSB partner of CC 6, so with a parameter selected it
//!   yields both a `Parameter` and a `ControlChange14` event (in that order).
//! - Parameter values are remembered per (channel, kind, number) in a fixed-size table of the
//!   [`PARAMETER_SLOTS`] most recently used parameters, so an LSB-only update or an increment of
//!   a parameter selected earlier reports its real current value.
//! - Data increment / decrement (CC 96 / 97) step the value by one unit: one LSB step for a
//!   parameter that has received a data entry LSB (a 14-bit parameter), otherwise one MSB step.
//!   The step saturates at the ends of the range.
//! - Channel mode messages 120 (all sound off) and 123..=127 (all notes off, omni/mono/poly)
//!   release every held note on their channel. 121 (reset all controllers) resets pitch bend,
//!   channel/poly pressure, modulation, expression, pedals 64..=67 and the parameter selection,
//!   as RP-015 prescribes (controllers that were never received stay unknown).
//! - [`MidiMessage::Reset`] forgets everything.
//! - `update` never allocates and never panics; messages with out-of-range fields (channel not
//!   in 1..=16, note above 127) are ignored, out-of-range values are clamped.

use std::fmt;

use crate::message::{
    ControlChange14, Derived, MidiMessage, ParameterChange, ParameterEvent, ParameterKind,
};

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

/// How many NRPN/RPN parameter values one [`MidiState`] remembers (least recently used ones are
/// forgotten first).
pub const PARAMETER_SLOTS: usize = 128;

/// "Never received" marker in the 7-bit value tables (valid values are 0..=127).
const UNSEEN: u8 = 0xFF;

const PITCH_BEND_CENTER: u16 = 8192;

#[derive(Clone, Copy)]
struct ChannelState {
    /// Velocity of each held note, 0 = not held.
    notes: [u8; 128],
    poly_pressure: [u8; 128],
    /// Last value of every controller, `UNSEEN` if never received.
    controls: [u8; 128],
    /// LSB half of the 14-bit pairs (controllers 0..=31); reset to 0 by their MSB.
    lsb14: [u8; 32],
    pitch_bend: u16,
    channel_pressure: u8,
    program: u8,
    /// Parameter number halves `[msb, lsb]` as last set by CC 99/98 and CC 101/100.
    nrpn: [u8; 2],
    rpn: [u8; 2],
    selected: Option<ParameterKind>,
}

impl ChannelState {
    const NEW: ChannelState = ChannelState {
        notes: [0; 128],
        poly_pressure: [0; 128],
        controls: [UNSEEN; 128],
        lsb14: [0; 32],
        pitch_bend: PITCH_BEND_CENTER,
        channel_pressure: 0,
        program: UNSEEN,
        nrpn: [0; 2],
        rpn: [0; 2],
        selected: None,
    };

    fn release_all_notes(&mut self) {
        self.notes = [0; 128];
        self.poly_pressure = [0; 128];
    }

    /// Reset All Controllers (CC 121) as specified by RP-015.
    fn reset_controllers(&mut self) {
        self.pitch_bend = PITCH_BEND_CENTER;
        self.channel_pressure = 0;
        self.poly_pressure = [0; 128];
        for (controller, value) in
            [(1, 0), (33, 0), (11, 127), (43, 0), (64, 0), (65, 0), (66, 0), (67, 0)]
        {
            if self.controls[controller] != UNSEEN {
                self.controls[controller] = value;
            }
        }
        self.lsb14[1] = 0;
        self.lsb14[11] = 0;
        self.nrpn = [127, 127];
        self.rpn = [127, 127];
        self.selected = None;
    }

    /// The selected parameter as (kind, 14-bit number).
    fn selected_parameter(&self) -> Option<(ParameterKind, u16)> {
        let kind = self.selected?;
        let [msb, lsb] = match kind {
            ParameterKind::Nrpn => self.nrpn,
            ParameterKind::Rpn => self.rpn,
        };
        Some((kind, (u16::from(msb) << 7) | u16::from(lsb)))
    }
}

/// One remembered parameter value.
#[derive(Clone, Copy)]
struct ParamSlot {
    /// `channel index << 15 | kind << 14 | number`.
    key: u32,
    msb: u8,
    lsb: u8,
    /// A data entry LSB was received for this parameter (it is used as a 14-bit value).
    hires: bool,
    /// Last use (from `MidiState::clock`); 0 = free slot.
    used: u64,
}

impl ParamSlot {
    const FREE: ParamSlot = ParamSlot { key: 0, msb: 0, lsb: 0, hires: false, used: 0 };
}

/// Tracks the state of one MIDI input port.
#[derive(Clone)]
pub struct MidiState {
    channels: [ChannelState; 16],
    params: [ParamSlot; PARAMETER_SLOTS],
    clock: u64,
}

impl Default for MidiState {
    fn default() -> Self {
        MidiState {
            channels: [ChannelState::NEW; 16],
            params: [ParamSlot::FREE; PARAMETER_SLOTS],
            clock: 0,
        }
    }
}

impl fmt::Debug for MidiState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let selected: Vec<(u8, ParameterKind, u16)> = (1..=16)
            .filter_map(|channel| {
                self.selected_parameter(channel).map(|(kind, number)| (channel, kind, number))
            })
            .collect();
        f.debug_struct("MidiState")
            .field("held_notes", &self.held_notes())
            .field("selected_parameters", &selected)
            .finish_non_exhaustive()
    }
}

/// `channel` (1..=16) as an array index.
fn index(channel: u8) -> Option<usize> {
    (1..=16).contains(&channel).then(|| usize::from(channel - 1))
}

impl MidiState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Applies a message and returns the derived high-level events it completes.
    pub fn update(&mut self, message: &MidiMessage) -> DerivedEvents {
        let mut out = DerivedEvents::default();
        match *message {
            MidiMessage::NoteOn { channel, note, velocity } => {
                if let (Some(ch), Some(n)) = (self.channel_mut(channel), note_index(note)) {
                    if velocity == 0 {
                        ch.notes[n] = 0;
                        ch.poly_pressure[n] = 0;
                    } else {
                        ch.notes[n] = velocity.min(127);
                    }
                }
            }
            MidiMessage::NoteOff { channel, note, .. } => {
                if let (Some(ch), Some(n)) = (self.channel_mut(channel), note_index(note)) {
                    ch.notes[n] = 0;
                    ch.poly_pressure[n] = 0;
                }
            }
            MidiMessage::PolyPressure { channel, note, pressure } => {
                if let (Some(ch), Some(n)) = (self.channel_mut(channel), note_index(note)) {
                    ch.poly_pressure[n] = pressure.min(127);
                }
            }
            MidiMessage::ControlChange { channel, controller, value } => {
                if let (Some(ci), true) = (index(channel), controller < 128) {
                    self.control_change(ci, channel, controller, value.min(127), &mut out);
                }
            }
            MidiMessage::ProgramChange { channel, program } => {
                if let Some(ch) = self.channel_mut(channel) {
                    ch.program = program.min(127);
                }
            }
            MidiMessage::ChannelPressure { channel, pressure } => {
                if let Some(ch) = self.channel_mut(channel) {
                    ch.channel_pressure = pressure.min(127);
                }
            }
            MidiMessage::PitchBend { channel, value } => {
                if let Some(ch) = self.channel_mut(channel) {
                    ch.pitch_bend = value.min(16383);
                }
            }
            MidiMessage::Reset => self.reset(),
            _ => {}
        }
        out
    }

    fn control_change(
        &mut self,
        ci: usize,
        channel: u8,
        controller: u8,
        value: u8,
        out: &mut DerivedEvents,
    ) {
        let ch = &mut self.channels[ci];
        ch.controls[usize::from(controller)] = value;

        let mut pair = None;
        match controller {
            0..=31 => ch.lsb14[usize::from(controller)] = 0,
            32..=63 => {
                let msb_controller = controller - 32;
                ch.lsb14[usize::from(msb_controller)] = value;
                let msb = ch.controls[usize::from(msb_controller)];
                if msb != UNSEEN {
                    pair = Some(ControlChange14 {
                        channel,
                        controller: msb_controller,
                        value: (u16::from(msb) << 7) | u16::from(value),
                    });
                }
            }
            _ => {}
        }

        match controller {
            6 | 38 | 96 | 97 => {
                if let Some(event) = self.data_entry(ci, channel, controller, value) {
                    out.push(Derived::Parameter(event));
                }
            }
            98 | 99 => {
                ch.nrpn[usize::from(controller == 98)] = value;
                ch.selected = Some(ParameterKind::Nrpn);
            }
            100 | 101 => {
                ch.rpn[usize::from(controller == 100)] = value;
                ch.selected = (ch.rpn != [127, 127]).then_some(ParameterKind::Rpn);
            }
            120 | 123..=127 => ch.release_all_notes(),
            121 => ch.reset_controllers(),
            _ => {}
        }

        if let Some(pair) = pair {
            out.push(Derived::ControlChange14(pair));
        }
    }

    /// Applies a data entry / increment / decrement CC to the selected parameter.
    fn data_entry(
        &mut self,
        ci: usize,
        channel: u8,
        controller: u8,
        value: u8,
    ) -> Option<ParameterEvent> {
        let (kind, number) = self.channels[ci].selected_parameter()?;
        let kind_bit = match kind {
            ParameterKind::Nrpn => 0,
            ParameterKind::Rpn => 1,
        };
        let key = ((ci as u32) << 15) | (kind_bit << 14) | u32::from(number);
        let slot = self.param_slot(key);
        let change = match controller {
            6 => {
                slot.msb = value;
                slot.lsb = 0;
                ParameterChange::DataMsb
            }
            38 => {
                slot.lsb = value;
                slot.hires = true;
                ParameterChange::DataLsb
            }
            96 | 97 => {
                let up = controller == 96;
                if slot.hires {
                    let current = (u16::from(slot.msb) << 7) | u16::from(slot.lsb);
                    let next =
                        if up { (current + 1).min(16383) } else { current.saturating_sub(1) };
                    slot.msb = (next >> 7) as u8;
                    slot.lsb = (next & 0x7F) as u8;
                } else {
                    slot.msb =
                        if up { (slot.msb + 1).min(127) } else { slot.msb.saturating_sub(1) };
                }
                if up { ParameterChange::Increment } else { ParameterChange::Decrement }
            }
            _ => return None,
        };
        Some(ParameterEvent {
            channel,
            kind,
            number,
            change,
            msb: slot.msb,
            lsb: slot.lsb,
            amount: value,
        })
    }

    /// The remembered value slot for `key`, claiming the least recently used slot if needed.
    fn param_slot(&mut self, key: u32) -> &mut ParamSlot {
        self.clock += 1;
        let mut chosen = 0;
        let mut oldest = u64::MAX;
        let mut found = false;
        for (i, slot) in self.params.iter().enumerate() {
            if slot.used != 0 && slot.key == key {
                chosen = i;
                found = true;
                break;
            }
            if slot.used < oldest {
                oldest = slot.used;
                chosen = i;
            }
        }
        let slot = &mut self.params[chosen];
        if !found {
            *slot = ParamSlot { key, ..ParamSlot::FREE };
        }
        slot.used = self.clock;
        slot
    }

    fn channel(&self, channel: u8) -> Option<&ChannelState> {
        index(channel).map(|i| &self.channels[i])
    }

    fn channel_mut(&mut self, channel: u8) -> Option<&mut ChannelState> {
        index(channel).map(|i| &mut self.channels[i])
    }

    /// Velocity of a held note, `0` when the note is not held.
    pub fn note_velocity(&self, channel: u8, note: u8) -> u8 {
        match (self.channel(channel), note_index(note)) {
            (Some(ch), Some(n)) => ch.notes[n],
            _ => 0,
        }
    }

    pub fn is_note_held(&self, channel: u8, note: u8) -> bool {
        self.note_velocity(channel, note) > 0
    }

    /// All currently held notes as `(channel, note, velocity)`.
    pub fn held_notes(&self) -> Vec<(u8, u8, u8)> {
        let mut held = Vec::new();
        for (ci, ch) in self.channels.iter().enumerate() {
            for (note, &velocity) in ch.notes.iter().enumerate() {
                if velocity > 0 {
                    held.push((ci as u8 + 1, note as u8, velocity));
                }
            }
        }
        held
    }

    /// Last received value of a controller, `None` if never seen.
    pub fn control(&self, channel: u8, controller: u8) -> Option<u8> {
        let ch = self.channel(channel)?;
        let value = *ch.controls.get(usize::from(controller))?;
        (value != UNSEEN).then_some(value)
    }

    /// Current 14-bit value of the controller pair `controller` (0..=31) / `controller + 32`:
    /// `msb << 7 | lsb`, where the LSB counts as 0 until it is received after the MSB.
    /// `None` if the MSB was never seen.
    pub fn control14(&self, channel: u8, controller: u8) -> Option<u16> {
        let ch = self.channel(channel)?;
        let lsb = *ch.lsb14.get(usize::from(controller))?;
        let msb = ch.controls[usize::from(controller)];
        (msb != UNSEEN).then(|| (u16::from(msb) << 7) | u16::from(lsb))
    }

    /// The NRPN / RPN currently selected on `channel` as (kind, 14-bit number).
    pub fn selected_parameter(&self, channel: u8) -> Option<(ParameterKind, u16)> {
        self.channel(channel)?.selected_parameter()
    }

    /// Last pitch bend value (`8192` if never seen).
    pub fn pitch_bend(&self, channel: u8) -> u16 {
        self.channel(channel).map_or(PITCH_BEND_CENTER, |ch| ch.pitch_bend)
    }

    pub fn channel_pressure(&self, channel: u8) -> u8 {
        self.channel(channel).map_or(0, |ch| ch.channel_pressure)
    }

    pub fn poly_pressure(&self, channel: u8, note: u8) -> u8 {
        match (self.channel(channel), note_index(note)) {
            (Some(ch), Some(n)) => ch.poly_pressure[n],
            _ => 0,
        }
    }

    /// Last received program, `None` if never seen.
    pub fn program(&self, channel: u8) -> Option<u8> {
        let program = self.channel(channel)?.program;
        (program != UNSEEN).then_some(program)
    }

    /// Forgets everything (use after a device disconnects or on MIDI reset).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

fn note_index(note: u8) -> Option<usize> {
    (note < 128).then_some(usize::from(note))
}
