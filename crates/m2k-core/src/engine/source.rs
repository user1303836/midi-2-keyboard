//! MIDI input side of the engine: per-device state and the vocabulary a mapping's source speaks
//! (see the module docs of [`crate::model`]: every source is *active* or *inactive* and carries a
//! value; pulses are both at once).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use m2k_midi::MidiState;

use super::timers::TimerId;
use crate::model::{ValueEncoding, ValueRange};

/// Index of a device in [`Devices`]. Stable for the life of the engine.
pub type DeviceId = u16;

/// Stands for "no device" (an action started from the UI, not by a message).
pub const NO_DEVICE: DeviceId = DeviceId::MAX;

/// One independently tracked thing a source can match: (device, channel, note / controller /
/// parameter number). Channel 0 is used for system messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InstKey(u64);

impl InstKey {
    pub fn new(device: DeviceId, channel: u8, number: u16) -> Self {
        InstKey((u64::from(device) << 32) | (u64::from(channel) << 16) | u64::from(number))
    }

    pub fn device(self) -> DeviceId {
        (self.0 >> 32) as DeviceId
    }

    pub fn channel(self) -> u8 {
        (self.0 >> 16) as u8
    }
}

/// What one message means for one mapping's source, before the mapping's own instance table
/// turns it into an edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Obs {
    /// The instance is active with this message (note on in range, chord complete).
    Active,
    /// The instance is inactive with this message (note off, forced release).
    Inactive,
    /// A ranged absolute value: whether it is inside the source's range, and inside the range
    /// widened by the source's hysteresis. An inactive instance activates only `inside`; an
    /// active one stays active while inside the `band`.
    Level { inside: bool, band: bool },
    /// Active and inactive at once (program change, SysEx, clock, ...).
    Pulse,
    /// Relative movement: signed ticks from an endless encoder or NRPN/RPN increment/decrement.
    Ticks(i32),
}

/// Decodes a relative-encoded CC value into signed ticks (0 = no movement).
pub fn relative_ticks(value: u8, encoding: ValueEncoding) -> i32 {
    let v = i32::from(value.min(127));
    match encoding {
        ValueEncoding::Absolute => 0,
        ValueEncoding::RelativeTwosComplement => match v {
            1..=63 => v,
            65..=127 => -(128 - v),
            _ => 0,
        },
        ValueEncoding::RelativeBinaryOffset => v - 64,
        ValueEncoding::RelativeSignMagnitude => match v {
            1..=63 => v,
            65..=127 => -(v - 64),
            _ => 0,
        },
    }
}

/// How long a device that sends Active Sensing may stay silent before it counts as lost (the
/// MIDI spec says 300 ms; a little slack for busy systems).
pub const SENSING_TIMEOUT: Duration = Duration::from_millis(350);

/// Everything the engine knows about one input device.
pub struct DeviceState {
    pub name: Arc<str>,
    pub midi: MidiState,
    /// Notes held per channel (bit per note). Kept by the engine because chords and the
    /// releases of pressure sources depend on which notes are down.
    held: [u128; 16],
    /// When each note was last struck (microseconds of engine time), for chord windows.
    struck: Box<[[u64; 128]; 16]>,
    /// MIDI clock ticks since the last Start (or since the clock was first seen).
    pub clock_ticks: u64,
    /// The device sends Active Sensing, so silence means the connection is gone (a cable pulled
    /// out of an interface whose port stays present).
    pub sensing: bool,
    /// When the last message from this device arrived.
    pub last_heard: Option<Instant>,
    pub sensing_timer: Option<TimerId>,
}

impl DeviceState {
    fn new(name: Arc<str>) -> Self {
        DeviceState {
            name,
            midi: MidiState::new(),
            held: [0; 16],
            struck: Box::new([[0; 128]; 16]),
            clock_ticks: 0,
            sensing: false,
            last_heard: None,
            sensing_timer: None,
        }
    }

    /// Forgets the device's MIDI state (the caller cancels `sensing_timer`).
    pub fn reset(&mut self) {
        self.midi.reset();
        self.held = [0; 16];
        self.clock_ticks = 0;
        self.sensing = false;
    }

    /// Records a note on (struck at `time_us`) or note off.
    pub fn set_note(&mut self, channel: u8, note: u8, on: bool, time_us: u64) {
        if let (Some(ci), true) = (channel_index(channel), note < 128) {
            let bit = 1u128 << note;
            if on {
                self.held[ci] |= bit;
                self.struck[ci][usize::from(note)] = time_us;
            } else {
                self.held[ci] &= !bit;
            }
        }
    }

    /// The notes held on `channel` (bit per note).
    pub fn held_notes(&self, channel: u8) -> u128 {
        channel_index(channel).map_or(0, |ci| self.held[ci])
    }

    /// When `note` was last struck on `channel` (engine time in microseconds).
    pub fn struck_at(&self, channel: u8, note: u8) -> u64 {
        match (channel_index(channel), note < 128) {
            (Some(ci), true) => self.struck[ci][usize::from(note)],
            _ => 0,
        }
    }

    /// Forgets the held notes of one channel (`None` = all channels).
    pub fn clear_notes(&mut self, channel: Option<u8>) {
        match channel {
            Some(channel) => {
                if let Some(ci) = channel_index(channel) {
                    self.held[ci] = 0;
                }
            }
            None => self.held = [0; 16],
        }
    }

    pub fn any_note_held(&self, channel: u8) -> bool {
        channel_index(channel).is_some_and(|ci| self.held[ci] != 0)
    }
}

fn channel_index(channel: u8) -> Option<usize> {
    (1..=16).contains(&channel).then(|| usize::from(channel - 1))
}

/// All devices seen since the engine started. Ids are never reused; a disconnect only resets
/// the device's state.
#[derive(Default)]
pub struct Devices {
    list: Vec<DeviceState>,
    by_name: HashMap<Arc<str>, DeviceId>,
}

impl Devices {
    pub fn id(&self, name: &str) -> Option<DeviceId> {
        self.by_name.get(name).copied()
    }

    pub fn id_or_insert(&mut self, name: &str) -> DeviceId {
        if let Some(id) = self.by_name.get(name) {
            return *id;
        }
        let id = self.list.len() as DeviceId;
        let name: Arc<str> = Arc::from(name);
        self.list.push(DeviceState::new(name.clone()));
        self.by_name.insert(name, id);
        id
    }

    /// The device's port name (`None` for [`NO_DEVICE`]).
    pub fn name(&self, id: DeviceId) -> Option<&Arc<str>> {
        self.list.get(usize::from(id)).map(|device| &device.name)
    }

    pub fn get(&self, id: DeviceId) -> &DeviceState {
        &self.list[usize::from(id)]
    }

    pub fn get_mut(&mut self, id: DeviceId) -> &mut DeviceState {
        &mut self.list[usize::from(id)]
    }

    /// After the timer queue was cleared: no device has a watchdog timer any more (they are
    /// re-armed by the next message).
    pub fn forget_timers(&mut self) {
        for device in &mut self.list {
            device.sensing_timer = None;
        }
    }

    /// [`crate::model::Condition::NoteHeld`]: the note is held on any device.
    pub fn note_held(&self, channel: Option<u8>, note: u8) -> bool {
        self.list.iter().any(|d| match channel {
            Some(channel) => d.midi.is_note_held(channel, note),
            None => (1..=16).any(|channel| d.midi.is_note_held(channel, note)),
        })
    }

    /// The controller's last value (first device / channel that has one), for messages.
    pub fn control_value(&self, channel: Option<u8>, controller: u8) -> Option<u8> {
        self.list.iter().find_map(|d| match channel {
            Some(channel) => d.midi.control(channel, controller),
            None => (1..=16).find_map(|channel| d.midi.control(channel, controller)),
        })
    }

    /// [`crate::model::Condition::ControlValue`]: the controller's last value on any device is
    /// inside `range`. False when it was never received.
    pub fn control_in(&self, channel: Option<u8>, controller: u8, range: ValueRange) -> bool {
        let inside = |d: &DeviceState, channel: u8| {
            d.midi.control(channel, controller).is_some_and(|v| range.contains(u16::from(v)))
        };
        self.list.iter().any(|d| match channel {
            Some(channel) => inside(d, channel),
            None => (1..=16).any(|channel| inside(d, channel)),
        })
    }
}
