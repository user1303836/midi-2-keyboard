//! The MIDI side of the engine thread: one message in, observations out to every live mapping
//! whose source can match it (via the [`SourceIndex`]), plus everything a message implies for
//! instances it does not name directly: chords, aftertouch of released notes, all-notes-off,
//! reset-all-controllers, MIDI reset, and the Active Sensing watchdog.

use std::sync::Arc;

use m2k_midi::{Derived, MidiMessage, NoteEvent, ParameterChange, ParameterKind};

use super::Worker;
use crate::engine::behavior;
use crate::engine::compile::{CSourceKind, SourceIndex, system_index};
use crate::engine::cx::{Ctx, TimerKind};
use crate::engine::source::{DeviceId, DeviceState, InstKey, Obs, SENSING_TIMEOUT, relative_ticks};
use crate::engine::timers::TimerId;
use crate::events::{EngineEvent, MidiLog};
use crate::model::{SystemMessage, ValueEncoding, ValueRange};

impl Worker {
    pub(super) fn on_midi(&mut self, device: String, message: MidiMessage) {
        let dev = self.cx.devices.id_or_insert(&device);
        let now = self.cx.now;
        let state = self.cx.devices.get_mut(dev);
        state.last_heard = Some(now);
        if message == MidiMessage::ActiveSensing {
            state.sensing = true;
        }
        if state.sensing && state.sensing_timer.is_none() {
            let timer = self.cx.timers.schedule(now + SENSING_TIMEOUT, TimerKind::Sensing(dev));
            self.cx.devices.get_mut(dev).sensing_timer = Some(timer);
        }
        let derived = self.cx.devices.get_mut(dev).midi.update(&message);
        let mark = self.cx.outbox.len();
        let mut matched = Vec::new();
        let index = Arc::clone(&self.index);
        self.dispatch(&index, dev, &device, &message, &mut matched);
        for event in derived.iter() {
            self.dispatch_derived(&index, dev, &device, event, &mut matched);
        }
        if !self.cx.monitor {
            return;
        }
        let log = MidiLog {
            time_us: self.cx.time_us(),
            device,
            message,
            derived: derived.iter().collect(),
            mappings: matched,
        };
        self.cx.outbox.insert(mark, EngineEvent::Midi(log));
    }

    /// Hands one observation to mapping `i` (if live) and runs its behavior.
    fn feed(&mut self, i: usize, inst: InstKey, obs: Obs, ctx: Ctx, matched: &mut Vec<String>) {
        let def = &self.cfg.mappings[i];
        let rt = &mut self.maps[i];
        if !rt.live {
            return;
        }
        let was_active = !rt.instances.is_empty();
        let Some(edge) = rt.observe(def, &mut self.cx, inst, obs, ctx) else {
            return;
        };
        if self.cx.monitor && !matched.iter().any(|id| id.as_str() == &*def.id) {
            matched.push(def.id.to_string());
        }
        // Feedback without a device of its own answers the device that last reached it.
        let touched = rt.fb.last_device != Some(inst.device());
        if touched {
            rt.fb.last_device = Some(inst.device());
        }
        let feedback_moved = touched && def.feedback.is_some();
        behavior::on_edge(rt, def, &mut self.cx, inst, edge, ctx, was_active);
        if feedback_moved {
            self.fb_due = true;
        }
    }

    /// Ranged absolute sources: active once the value enters the range, inactive once it
    /// leaves it (widened by the source's hysteresis).
    #[allow(clippy::too_many_arguments)]
    fn level(
        &mut self,
        list: &[usize],
        name: &str,
        channel: Option<u8>,
        inst: InstKey,
        value: u16,
        ctx: Ctx,
        matched: &mut Vec<String>,
        range_of: impl Fn(&CSourceKind) -> Option<ValueRange>,
    ) {
        for &i in list {
            let source = &self.cfg.mappings[i].source;
            if !source.accepts(name, channel) {
                continue;
            }
            let Some(range) = range_of(&source.kind) else {
                continue;
            };
            let obs = source.level(range, value);
            self.feed(i, inst, obs, ctx, matched);
        }
    }

    /// Pulse sources (press and release at once) whose filter accepts the message.
    #[allow(clippy::too_many_arguments)]
    fn pulse(
        &mut self,
        list: &[usize],
        name: &str,
        channel: Option<u8>,
        inst: InstKey,
        ctx: Ctx,
        matched: &mut Vec<String>,
        accept: impl Fn(&CSourceKind) -> bool,
    ) {
        for &i in list {
            let source = &self.cfg.mappings[i].source;
            if !source.accepts(name, channel) || !accept(&source.kind) {
                continue;
            }
            self.feed(i, inst, Obs::Pulse, ctx, matched);
        }
    }

    fn dispatch(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        name: &str,
        message: &MidiMessage,
        matched: &mut Vec<String>,
    ) {
        let base = Ctx { device: dev, ..Ctx::default() };
        let system = InstKey::new(dev, 0, 0);
        match *message {
            MidiMessage::NoteOn { .. } | MidiMessage::NoteOff { .. } => {
                if let Some(event) = message.note_event() {
                    self.on_note(index, dev, name, event, matched);
                }
            }
            MidiMessage::ControlChange { channel, controller, value } => {
                let ctx = Ctx {
                    channel,
                    controller: Some(u16::from(controller)),
                    value: i32::from(value),
                    value7: value,
                    ..base
                };
                let inst = InstKey::new(dev, channel, u16::from(controller));
                for &i in &index.cc[usize::from(controller & 127)] {
                    let source = &self.cfg.mappings[i].source;
                    let CSourceKind::Cc { range, encoding, .. } = &source.kind else {
                        continue;
                    };
                    if !source.accepts(name, Some(channel)) {
                        continue;
                    }
                    let (obs, ctx) = match encoding {
                        ValueEncoding::Absolute => (source.level(*range, u16::from(value)), ctx),
                        relative => {
                            let ticks = relative_ticks(value, *relative);
                            if ticks == 0 {
                                continue;
                            }
                            (Obs::Ticks(ticks), Ctx { value: ticks, ..ctx })
                        }
                    };
                    self.feed(i, inst, obs, ctx, matched);
                }
                match controller {
                    // All sound off, all notes off, omni/mono/poly: every note is released.
                    120 | 123..=127 => self.release_notes(index, dev, Some(channel), matched),
                    121 => self.reset_controllers(index, dev, channel, matched),
                    _ => {}
                }
            }
            MidiMessage::ProgramChange { channel, program } => {
                let ctx = Ctx { channel, value: i32::from(program), value7: program, ..base };
                let inst = InstKey::new(dev, channel, 0);
                let program = u16::from(program);
                self.pulse(&index.program, name, Some(channel), inst, ctx, matched, |kind| {
                    matches!(kind, CSourceKind::Program { programs } if programs.contains(program))
                });
            }
            MidiMessage::PitchBend { channel, value } => {
                let ctx =
                    Ctx { channel, value: i32::from(value), value7: (value >> 7) as u8, ..base };
                let inst = InstKey::new(dev, channel, 0);
                self.level(
                    &index.pitch_bend,
                    name,
                    Some(channel),
                    inst,
                    value,
                    ctx,
                    matched,
                    |k| match k {
                        CSourceKind::PitchBend { range } => Some(*range),
                        _ => None,
                    },
                );
            }
            MidiMessage::ChannelPressure { channel, pressure } => {
                let ctx = Ctx { channel, value: i32::from(pressure), value7: pressure, ..base };
                let inst = InstKey::new(dev, channel, 0);
                let list = &index.channel_pressure;
                let value = u16::from(pressure);
                self.level(list, name, Some(channel), inst, value, ctx, matched, |k| match k {
                    CSourceKind::ChannelPressure { range } => Some(*range),
                    _ => None,
                });
            }
            MidiMessage::PolyPressure { channel, note, pressure } => {
                let value = i32::from(pressure);
                let ctx = Ctx { channel, note: Some(note), value, value7: pressure, ..base };
                let inst = InstKey::new(dev, channel, u16::from(note));
                let list = &index.poly_pressure;
                let value = u16::from(pressure);
                self.level(list, name, Some(channel), inst, value, ctx, matched, |k| match k {
                    CSourceKind::PolyPressure { notes, range }
                        if notes.contains(u16::from(note)) =>
                    {
                        Some(*range)
                    }
                    _ => None,
                });
            }
            MidiMessage::SysEx { ref data } => {
                self.pulse(&index.sysex, name, None, system, base, matched, |kind| {
                    matches!(kind, CSourceKind::SysEx { pattern: Some(p) } if p.matches(data))
                });
            }
            MidiMessage::Clock => {
                let state = self.cx.devices.get_mut(dev);
                let tick = state.clock_ticks;
                state.clock_ticks += 1;
                self.pulse(&index.clock, name, None, system, base, matched, |kind| {
                    matches!(kind, CSourceKind::Clock { divisor }
                        if tick.is_multiple_of(u64::from((*divisor).max(1))))
                });
            }
            MidiMessage::SongSelect { song } => {
                let ctx = Ctx { value: i32::from(song), value7: song, ..base };
                let song = u16::from(song);
                self.pulse(&index.song_select, name, None, system, ctx, matched, |kind| {
                    matches!(kind, CSourceKind::SongSelect { songs } if songs.contains(song))
                });
            }
            MidiMessage::SongPosition { position } => {
                let ctx = Ctx { value: i32::from(position), value7: (position >> 7) as u8, ..base };
                let list = &index.song_position;
                self.level(list, name, None, system, position, ctx, matched, |k| match k {
                    CSourceKind::SongPosition { range } => Some(*range),
                    _ => None,
                });
            }
            MidiMessage::Start => {
                self.cx.devices.get_mut(dev).clock_ticks = 0;
                self.system(index, SystemMessage::Start, name, system, base, matched);
            }
            MidiMessage::Continue => {
                self.system(index, SystemMessage::Continue, name, system, base, matched)
            }
            MidiMessage::Stop => {
                self.system(index, SystemMessage::Stop, name, system, base, matched)
            }
            MidiMessage::TuneRequest => {
                self.system(index, SystemMessage::TuneRequest, name, system, base, matched)
            }
            MidiMessage::ActiveSensing => {
                self.system(index, SystemMessage::ActiveSensing, name, system, base, matched)
            }
            MidiMessage::Reset => {
                self.system(index, SystemMessage::Reset, name, system, base, matched);
                // A reset device has no notes down and its controllers are at their defaults.
                self.release_notes(index, dev, None, matched);
                for channel in 1..=16 {
                    self.reset_controllers(index, dev, channel, matched);
                }
                self.cx.devices.get_mut(dev).clock_ticks = 0;
            }
            MidiMessage::MtcQuarterFrame { .. } | MidiMessage::Unknown { .. } => {}
        }
    }

    fn on_note(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        name: &str,
        event: NoteEvent,
        matched: &mut Vec<String>,
    ) {
        let NoteEvent { channel, note, on, velocity } = event;
        let time_us = self.cx.time_us();
        self.cx.devices.get_mut(dev).set_note(channel, note, on, time_us);
        let ctx = Ctx {
            device: dev,
            channel,
            note: Some(note),
            velocity: Some(velocity),
            value: i32::from(velocity),
            value7: velocity,
            ..Ctx::default()
        };
        let inst = InstKey::new(dev, channel, u16::from(note));
        for &i in &index.note_by[usize::from(note & 127)] {
            let source = &self.cfg.mappings[i].source;
            let CSourceKind::Note { notes, velocity: accepted } = &source.kind else {
                continue;
            };
            if !source.accepts(name, Some(channel)) || !notes.contains(u16::from(note)) {
                continue;
            }
            let obs = if !on {
                Obs::Inactive
            } else if accepted.contains(u16::from(velocity)) {
                Obs::Active
            } else {
                continue;
            };
            self.feed(i, inst, obs, ctx, matched);
        }
        if !index.chord.is_empty() {
            self.chords(index, dev, name, ctx, on, matched);
        }
        if !on {
            self.after_note_off(index, dev, channel, note, matched);
        }
    }

    /// Chords, per (device, channel): active while all of their notes are down; pressed by the
    /// note that completes them (within `window_ms` of the earliest held one when set),
    /// released by any of them going up.
    fn chords(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        name: &str,
        ctx: Ctx,
        on: bool,
        matched: &mut Vec<String>,
    ) {
        let (Some(note), channel) = (ctx.note, ctx.channel) else {
            return;
        };
        let bit = 1u128 << (note & 127);
        let inst = InstKey::new(dev, channel, 0);
        for &i in &index.chord {
            let source = &self.cfg.mappings[i].source;
            let CSourceKind::Chord { notes, mask, window_ms } = &source.kind else {
                continue;
            };
            if mask & bit == 0 || !source.accepts(name, Some(channel)) {
                continue;
            }
            let state = self.cx.devices.get(dev);
            let obs = if !on {
                Obs::Inactive
            } else if chord_complete(state, channel, note, notes, *mask, *window_ms) {
                Obs::Active
            } else {
                continue;
            };
            self.feed(i, inst, obs, ctx, matched);
        }
    }

    fn system(
        &mut self,
        index: &SourceIndex,
        message: SystemMessage,
        name: &str,
        inst: InstKey,
        ctx: Ctx,
        matched: &mut Vec<String>,
    ) {
        self.pulse(&index.system[system_index(message)], name, None, inst, ctx, matched, |_| true);
    }

    fn dispatch_derived(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        name: &str,
        event: Derived,
        matched: &mut Vec<String>,
    ) {
        match event {
            Derived::Parameter(param) => {
                let list = match param.kind {
                    ParameterKind::Nrpn => &index.nrpn,
                    ParameterKind::Rpn => &index.rpn,
                };
                let base = Ctx {
                    device: dev,
                    channel: param.channel,
                    controller: Some(param.number),
                    value7: param.value7(),
                    ..Ctx::default()
                };
                let inst = InstKey::new(dev, param.channel, param.number);
                for &i in list {
                    let source = &self.cfg.mappings[i].source;
                    let CSourceKind::Param { number, range, hires, .. } = &source.kind else {
                        continue;
                    };
                    if *number != param.number || !source.accepts(name, Some(param.channel)) {
                        continue;
                    }
                    let absolute = |value: u16| {
                        (source.level(*range, value), Ctx { value: i32::from(value), ..base })
                    };
                    let (obs, ctx) = match param.change {
                        ParameterChange::DataMsb if !*hires => absolute(u16::from(param.value7())),
                        ParameterChange::DataLsb if *hires => absolute(param.value14()),
                        ParameterChange::Increment => (Obs::Ticks(1), Ctx { value: 1, ..base }),
                        ParameterChange::Decrement => (Obs::Ticks(-1), Ctx { value: -1, ..base }),
                        _ => continue,
                    };
                    self.feed(i, inst, obs, ctx, matched);
                }
            }
            Derived::ControlChange14(cc) => {
                let ctx = Ctx {
                    device: dev,
                    channel: cc.channel,
                    controller: Some(u16::from(cc.controller)),
                    value: i32::from(cc.value),
                    value7: (cc.value >> 7) as u8,
                    ..Ctx::default()
                };
                let inst = InstKey::new(dev, cc.channel, u16::from(cc.controller));
                let list = &index.cc14[usize::from(cc.controller & 31)];
                self.level(
                    list,
                    name,
                    Some(cc.channel),
                    inst,
                    cc.value,
                    ctx,
                    matched,
                    |k| match k {
                        CSourceKind::Cc14 { range, .. } => Some(*range),
                        _ => None,
                    },
                );
            }
        }
    }

    /// After a note off: the released note's aftertouch is gone, and channel pressure cannot
    /// outlast the last held note of its channel (some keyboards never send pressure 0).
    fn after_note_off(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        channel: u8,
        note: u8,
        matched: &mut Vec<String>,
    ) {
        let ctx = Ctx { device: dev, channel, note: Some(note), ..Ctx::default() };
        let inst = InstKey::new(dev, channel, u16::from(note));
        for &i in &index.poly_pressure {
            self.feed(i, inst, Obs::Inactive, ctx, matched);
        }
        if !self.cx.devices.get(dev).any_note_held(channel) {
            let inst = InstKey::new(dev, channel, 0);
            let ctx = Ctx { note: None, ..ctx };
            for &i in &index.channel_pressure {
                self.feed(i, inst, Obs::Inactive, ctx, matched);
            }
        }
    }

    /// Releases every active note, chord and aftertouch instance of the device, on one channel
    /// or all, exactly as if the note offs had arrived.
    fn release_notes(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        channel: Option<u8>,
        matched: &mut Vec<String>,
    ) {
        self.cx.devices.get_mut(dev).clear_notes(channel);
        let lists = [&index.note, &index.chord, &index.poly_pressure, &index.channel_pressure];
        for &i in lists.into_iter().flatten() {
            let active = self.maps[i]
                .active(|key| key.device() == dev && channel.is_none_or(|c| key.channel() == c));
            for (key, ctx) in active {
                let ctx = Ctx { velocity: ctx.velocity.map(|_| 0), value: 0, value7: 0, ..ctx };
                self.feed(i, key, Obs::Inactive, ctx, matched);
            }
        }
    }

    /// Reset All Controllers (CC 121): controls that reset to a value outside a mapping's range
    /// (and its hysteresis band) release, as if that value had arrived. The RP-015 set, as
    /// tracked by `MidiState`.
    fn reset_controllers(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        channel: u8,
        matched: &mut Vec<String>,
    ) {
        self.release_outside(&index.pitch_bend, dev, channel, None, 8192, matched);
        self.release_outside(&index.channel_pressure, dev, channel, None, 0, matched);
        self.release_outside(&index.poly_pressure, dev, channel, None, 0, matched);
        for (controller, value) in [(1u16, 0u16), (11, 127), (64, 0), (65, 0), (66, 0), (67, 0)] {
            let list = &index.cc[usize::from(controller)];
            self.release_outside(list, dev, channel, Some(controller), value, matched);
        }
        for (controller, value) in [(1u16, 0u16), (11, 127 << 7)] {
            let list = &index.cc14[usize::from(controller)];
            self.release_outside(list, dev, channel, Some(controller), value, matched);
        }
    }

    fn release_outside(
        &mut self,
        list: &[usize],
        dev: DeviceId,
        channel: u8,
        number: Option<u16>,
        value: u16,
        matched: &mut Vec<String>,
    ) {
        for &i in list {
            let source = &self.cfg.mappings[i].source;
            let range = match &source.kind {
                CSourceKind::PitchBend { range }
                | CSourceKind::ChannelPressure { range }
                | CSourceKind::PolyPressure { range, .. }
                | CSourceKind::Cc { range, encoding: ValueEncoding::Absolute, .. }
                | CSourceKind::Cc14 { range, .. } => *range,
                _ => continue,
            };
            if matches!(source.level(range, value), Obs::Level { band: true, .. }) {
                continue;
            }
            let active = self.maps[i].active(|key| key.device() == dev && key.channel() == channel);
            for (key, ctx) in active {
                if number.is_some_and(|n| ctx.controller != Some(n)) {
                    continue;
                }
                let value7 = if value > 127 { (value >> 7) as u8 } else { value as u8 };
                let ctx = Ctx { value: i32::from(value), value7, ..ctx };
                self.feed(i, key, Obs::Inactive, ctx, matched);
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Devices
    // -----------------------------------------------------------------------------------------

    pub(super) fn device_disconnected(&mut self, name: &str) {
        if let Some(device) = self.cx.devices.id(name) {
            self.release_device(device);
        }
    }

    /// Active Sensing watchdog: a device that sends it and then goes silent has lost its
    /// connection (MIDI spec: receivers turn their notes off). Release what it was holding.
    pub(super) fn on_sensing_timer(&mut self, dev: DeviceId, id: TimerId) {
        let state = self.cx.devices.get_mut(dev);
        if state.sensing_timer != Some(id) {
            return;
        }
        state.sensing_timer = None;
        if !state.sensing {
            return;
        }
        let deadline = state.last_heard.map(|at| at + SENSING_TIMEOUT);
        if let Some(deadline) = deadline
            && deadline > self.cx.now
        {
            let timer = self.cx.timers.schedule(deadline, TimerKind::Sensing(dev));
            self.cx.devices.get_mut(dev).sensing_timer = Some(timer);
            return;
        }
        let name = state.name.clone();
        if self.release_device(dev) {
            self.cx.error(format!(
                "\"{name}\" stopped sending (its Active Sensing went silent; was a cable \
                 unplugged?). Everything it was holding has been released."
            ));
        }
    }

    /// Forgets a device's state and releases whatever its notes and controls were holding.
    /// Returns whether it was holding anything.
    fn release_device(&mut self, dev: DeviceId) -> bool {
        let mut released = false;
        for i in 0..self.maps.len() {
            released |=
                behavior::drop_device(&mut self.maps[i], &self.cfg.mappings[i], &mut self.cx, dev);
        }
        let state = self.cx.devices.get_mut(dev);
        state.reset();
        let mut timer = state.sensing_timer.take();
        self.cx.timers.cancel_slot(&mut timer);
        self.cx.dirty = true;
        // Lights that answered it look for their output again.
        self.fb_due = true;
        released
    }
}

/// All of a chord's notes are down on `channel` and, with a window, were struck within
/// `window_ms` of each other (from the earliest held one to `last`, the note just struck).
fn chord_complete(
    state: &DeviceState,
    channel: u8,
    last: u8,
    notes: &[u8],
    mask: u128,
    window_ms: u32,
) -> bool {
    if state.held_notes(channel) & mask != mask {
        return false;
    }
    if window_ms == 0 {
        return true;
    }
    let first = notes.iter().map(|n| state.struck_at(channel, *n)).min().unwrap_or(0);
    state.struck_at(channel, last).saturating_sub(first) <= u64::from(window_ms) * 1000
}
