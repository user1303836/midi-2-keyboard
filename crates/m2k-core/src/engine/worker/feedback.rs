//! Feedback: MIDI sent back to controllers so their lights (motor faders, displays) show what the
//! mappings do ([`crate::model::Feedback`]).
//!
//! After every event that may have changed something, each mapping with feedback compares the
//! state it shows (engaged, a layer, the effective profile) with what it last sent, and only a
//! change goes out. A refresh sends the current state again even when it did not change: when
//! the set of live mappings changes (a mapping that stops being live sends its `off`, before the
//! newly live ones send theirs), when output ports (re)appear, on resume and with a new MIDI
//! sink. A config install sends the state of new mappings and of those whose feedback changed
//! (the old light's `off` first), and the `off` of lit mappings it removes. Nothing is sent while paused or on exit: lights keep showing the
//! state. Feedback goes out after the key output of the same event, through the sink (which never
//! blocks), and before the event's monitor rows are delivered.

use std::sync::Arc;

use super::Worker;
use crate::engine::behavior::FbTarget;
use crate::engine::compile::{CFeedback, CMapping, CWhen};
use crate::engine::cx::{Cx, OwnerInfo};
use crate::engine::midi_out::CSend;
use crate::engine::source::DeviceId;
use crate::events::Outcome;

/// The last `off` owed by a mapping that was removed or changed, or whose feedback changed.
pub(super) struct FinalOff {
    pub info: OwnerInfo,
    pub port: Arc<str>,
    pub off: CSend,
}

/// Where a mapping's feedback goes: (1) its own output; (2) the output paired with its source
/// device; (3) the output paired with the device that last reached it; (4) before anything
/// reached it, the output paired with the connected input when exactly one connected input has
/// one. `None`: nowhere yet.
fn resolve(
    cx: &Cx,
    def: &CMapping,
    fb: &CFeedback,
    last: Option<DeviceId>,
    inputs: &[Arc<str>],
) -> Option<FbTarget> {
    if let Some(port) = &fb.device {
        let present = cx.midi.has_port(port);
        return Some(if present {
            FbTarget::Port(port.clone())
        } else {
            FbTarget::Missing(port.clone())
        });
    }
    let input: Arc<str> = match (&def.source.device, last.and_then(|d| cx.devices.name(d))) {
        (Some(device), _) => Arc::from(&**device),
        (None, Some(name)) => name.clone(),
        (None, None) => {
            let mut paired = inputs.iter().filter_map(|input| cx.midi.paired_output(input));
            let only = paired.next()?;
            return paired.next().is_none().then(|| FbTarget::Port(Arc::from(only)));
        }
    };
    Some(match cx.midi.paired_output(&input) {
        Some(output) if *output == *input => FbTarget::Port(input),
        Some(output) => FbTarget::Port(Arc::from(output)),
        None => FbTarget::Missing(input),
    })
}

impl Worker {
    /// Sends what changed since the last pass, and what a refresh asked for.
    pub(super) fn feedback_pass(&mut self) {
        if !self.fb_due && !self.cx.dirty {
            return;
        }
        if !self.fb_enabled || self.cx.gate.paused {
            // Kept due: resuming refreshes everything anyway.
            return;
        }
        self.fb_due = false;
        for owed in std::mem::take(&mut self.fb_final) {
            self.send_feedback(&owed.info, &FbTarget::Port(owed.port), &owed.off, false);
        }
        if self.feedback.is_empty() {
            return;
        }
        let effective = self.profiles.effective.map(|i| self.cfg.profiles[i].id.clone());
        // Mappings that are not live first (their `off`), then the live ones: a pad shared by
        // two profiles ends up showing the new one.
        for live in [false, true] {
            for k in 0..self.feedback.len() {
                let i = self.feedback[k];
                if self.maps[i].live == live {
                    self.feedback_of(i, effective.as_deref());
                }
            }
        }
    }

    fn feedback_of(&mut self, i: usize, effective: Option<&str>) {
        let generation = self.outputs_generation;
        let def = &self.cfg.mappings[i];
        let Some(fb) = &def.feedback else {
            return;
        };
        let rt = &mut self.maps[i];
        let state = rt.live
            && match &fb.when {
                CWhen::Engaged => rt.engaged(),
                CWhen::Layer(layer) => self.cx.layers.is_active(layer),
                CWhen::Profile(id) => effective == Some(&**id),
            };
        let last_device = rt.fb.last_device;
        let cached =
            rt.fb.target.as_ref().filter(|(g, d, _)| *g == generation && *d == last_device);
        if let Some((_, _, target)) = cached {
            // Fast path: same output as last time, same state.
            if !rt.fb.force && rt.fb.sent.as_ref().is_some_and(|(_, sent)| *sent == state) {
                return;
            }
            if target.is_none() {
                rt.fb.force = false;
                return;
            }
        }
        let target = match cached {
            Some((_, _, target)) => target.clone(),
            None => {
                let target = resolve(&self.cx, def, fb, last_device, &self.inputs);
                rt.fb.target = Some((generation, last_device, target.clone()));
                target
            }
        };
        let force = std::mem::take(&mut rt.fb.force);
        let Some(target) = target else {
            return;
        };
        let port = target.name().clone();
        let last = rt.fb.sent.replace((port.clone(), state));
        if !force && last.as_ref().is_some_and(|(p, s)| *p == port && *s == state) {
            return;
        }
        let info = rt.info.clone();
        let (on, off) = (fb.on.clone(), fb.off.clone());
        // The light moved to another output: switch the old one off.
        if let Some((old, true)) = &last
            && *old != port
            && let Some(off) = &off
        {
            self.send_feedback(&info, &FbTarget::Port(old.clone()), off, false);
        }
        if let Some(message) = if state { &on } else { &off } {
            self.send_feedback(&info, &target, message, state);
        }
    }

    /// One feedback send, reported like a firing: `light on: note C2 at 127 (ch 1) → "Pad"`. A
    /// missing output is reported once, until it appears or the config changes.
    fn send_feedback(&mut self, info: &OwnerInfo, target: &FbTarget, message: &CSend, on: bool) {
        // `Incoming` stands for 127 in `on` and 0 in `off`.
        let incoming = if on { 127 } else { 0 };
        let text = message.text(incoming, self.cx.settings.middle_c_octave);
        let verb = if on { "light on" } else { "light off" };
        let description = format!("{verb}: {text} \u{2192} \"{}\"", target.name());
        match target {
            FbTarget::Port(port) => match message.send(self.cx.midi.as_ref(), port, incoming) {
                Ok(()) => self.cx.report(info, description, Outcome::Sent),
                Err(error) => self.cx.report_failed_once(info, description, error),
            },
            FbTarget::Missing(port) => {
                if self.fb_missing.insert(port.clone()) {
                    let error = format!("there is no MIDI output named \"{port}\"");
                    self.cx.report(info, description, Outcome::Failed { error });
                }
            }
        }
    }

    /// Every live mapping with feedback sends its current state at the next pass.
    pub(super) fn refresh_feedback(&mut self) {
        for &i in &self.feedback {
            if self.maps[i].live {
                self.maps[i].fb.force = true;
            }
        }
        self.fb_due = true;
    }

    /// Output ports appeared, came back or changed: the feedback going to them is sent again.
    pub(super) fn midi_outputs_changed(&mut self, ports: Vec<String>) {
        self.outputs_generation += 1;
        let ports: Vec<Arc<str>> = ports.into_iter().map(Arc::from).collect();
        for port in &ports {
            self.fb_missing.remove(port);
        }
        let mut refresh = Vec::new();
        for &i in &self.feedback {
            let (def, rt) = (&self.cfg.mappings[i], &self.maps[i]);
            if let (true, Some(fb)) = (rt.live, &def.feedback)
                && let Some(target) = resolve(&self.cx, def, fb, rt.fb.last_device, &self.inputs)
                && ports.contains(target.name())
            {
                refresh.push(i);
            }
        }
        for i in refresh {
            self.maps[i].fb.force = true;
        }
        self.fb_due = true;
    }

    /// Before a config install drops mapping `j` (or replaces its feedback): owe its `off` if
    /// its light is on.
    pub(super) fn owe_final_off(&mut self, j: usize) {
        if let Some(fb) = &self.cfg.mappings[j].feedback
            && let Some(off) = &fb.off
            && let Some((port, true)) = &self.maps[j].fb.sent
        {
            let info = self.maps[j].info.clone();
            self.fb_final.push(FinalOff { info, port: port.clone(), off: off.clone() });
        }
    }
}
