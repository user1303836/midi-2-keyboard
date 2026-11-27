//! The shared mutable context of the engine thread ([`Cx`]): timers, output, action queues,
//! layers, gating and the outgoing event buffer. Behaviors receive it next to their own mapping
//! state; everything that has to restructure the engine (profile switches, pause) is queued as a
//! [`Request`] and applied by the runtime once the current message or timer is done.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use m2k_output::{MouseButton, OutputError};

use super::compile::{CAction, CCondition, CHoldTarget, CMidi, Class, HoldWhat};
use super::executor::{Enqueued, Executor, Typematic};
use super::launch::Launcher;
use super::midi_out::MidiSink;
use super::output::Output;
use super::pattern::Foreground;
use super::source::{DeviceId, Devices, NO_DEVICE};
use super::timers::{TimerId, Timers};
use crate::events::{EngineEvent, FiredLog, Outcome, SkipReason};
use crate::model::{EngineOp, LayerOp, ProfileTarget, Settings};

/// Data of the message that triggered something (for text placeholders and the monitor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ctx {
    /// [`NO_DEVICE`] when nothing came from a device (the UI's Test button).
    pub device: DeviceId,
    /// 1..=16, 0 for system messages.
    pub channel: u8,
    pub note: Option<u8>,
    pub velocity: Option<u8>,
    /// CC number, 14-bit CC MSB number or NRPN/RPN parameter number.
    pub controller: Option<u16>,
    /// The value (velocity, CC value, 14-bit value, program, signed ticks, ...).
    pub value: i32,
    /// The message's value as 7 bits (a 14-bit value's upper 7, a relative encoder's raw
    /// byte): what "Send MIDI" passes on as the incoming value.
    pub value7: u8,
}

impl Default for Ctx {
    fn default() -> Self {
        Ctx {
            device: NO_DEVICE,
            channel: 0,
            note: None,
            velocity: None,
            controller: None,
            value: 0,
            value7: 0,
        }
    }
}

/// Owner key of actions started by [`crate::Engine::run_action`].
pub const TEST_OWNER: u64 = 0;

/// Who is doing something, for reference counting and for the monitor.
#[derive(Debug, Clone)]
pub struct OwnerInfo {
    pub owner: u64,
    pub profile_id: Arc<str>,
    pub mapping_id: Arc<str>,
}

impl OwnerInfo {
    pub fn test() -> Self {
        OwnerInfo { owner: TEST_OWNER, profile_id: Arc::from(""), mapping_id: Arc::from("") }
    }
}

/// What a timer is for. The payload identifies the owner; the owner compares the fired
/// [`TimerId`] with the one it stored, so stale timers are ignored.
#[derive(Debug, Clone, Copy)]
pub enum TimerKind {
    /// Trigger auto-fire (mapping key).
    Repeat(u64),
    TapHold(u64),
    MultiTap(u64),
    /// A multi-tap press reached `hold_ms`.
    LongPress(u64),
    /// A debounce window of one source instance ended.
    Debounce(u64, super::source::InstKey),
    Limiter(u64, super::limiter::Slot),
    /// Key stagger of a hold engagement (owner key).
    Hold(u64),
    /// Next step of an owner's action queue.
    Queue(u64),
    /// Emulated keyboard auto-repeat.
    Typematic,
    /// A delayed `run_action`.
    RunAction(u64),
    /// Active Sensing watchdog of a device.
    Sensing(DeviceId),
    /// Retry releasing keys / buttons the backend refused to release.
    RetryStuck,
    /// Latency statistics are due for publication.
    Latency,
    /// Coalesced status emission.
    Status,
}

/// Engine-wide changes requested by actions, applied after the current event.
#[derive(Debug, Clone)]
pub enum Request {
    /// With the index of the selectable profile to switch to, for `ProfileTarget::ByValue`.
    SwitchProfile(ProfileTarget, OwnerInfo, Option<usize>),
    Engine(EngineOp),
}

/// Output gating state.
#[derive(Debug, Clone, Copy, Default)]
pub struct Gate {
    pub paused: bool,
    pub window_focused: bool,
    pub suspend_while_focused: bool,
    pub learning: bool,
}

impl Gate {
    pub fn focus_suspended(&self) -> bool {
        self.window_focused && self.suspend_while_focused
    }

    /// Something is suppressing OS output.
    pub fn blocks_os(&self) -> bool {
        self.learning || self.paused || self.focus_suspended()
    }

    /// Why something of `class` must not run now, if it must not.
    pub fn check(&self, class: Class) -> Option<SkipReason> {
        if self.learning {
            return Some(SkipReason::Learning);
        }
        if self.paused && class != Class::Engine {
            return Some(SkipReason::Paused);
        }
        if self.focus_suspended() && class == Class::Os {
            return Some(SkipReason::WindowFocused);
        }
        None
    }
}

/// Named layers: latched by [`crate::model::Action::Layer`], or held (reference counted) by
/// [`crate::model::HoldTarget::Layer`].
#[derive(Debug, Default)]
pub struct Layers {
    latched: BTreeSet<Arc<str>>,
    held: BTreeMap<Arc<str>, u32>,
}

impl Layers {
    pub fn is_active(&self, name: &str) -> bool {
        self.latched.contains(name) || self.held.contains_key(name)
    }

    /// Applies a latch operation; `true` if the set of active layers changed.
    pub fn apply(&mut self, name: &Arc<str>, op: LayerOp) -> bool {
        let before = self.is_active(name);
        match op {
            LayerOp::On => {
                self.latched.insert(name.clone());
            }
            LayerOp::Off => {
                self.latched.remove(&**name);
            }
            LayerOp::Toggle => {
                if !self.latched.remove(&**name) {
                    self.latched.insert(name.clone());
                }
            }
        }
        before != self.is_active(name)
    }

    pub fn hold(&mut self, name: &Arc<str>) -> bool {
        let before = self.is_active(name);
        *self.held.entry(name.clone()).or_insert(0) += 1;
        !before
    }

    pub fn unhold(&mut self, name: &str) -> bool {
        if let Some(count) = self.held.get_mut(name) {
            *count -= 1;
            if *count == 0 {
                self.held.remove(name);
                return !self.is_active(name);
            }
        }
        false
    }

    /// Forgets every hold (their holders were reset); latched layers stay.
    pub fn release_held(&mut self) {
        self.held.clear();
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.latched.iter().map(|n| n.to_string()).collect();
        for name in self.held.keys() {
            if !self.latched.contains(&**name) {
                names.push(name.to_string());
            }
        }
        names.sort();
        names
    }
}

/// Remembers recently reported failures so a broken backend does not flood the monitor.
#[derive(Debug, Default)]
struct FailureLog {
    recent: Vec<(u64, String, Instant)>,
}

impl FailureLog {
    const WINDOW: Duration = Duration::from_secs(2);

    /// `true` when this exact failure was reported for this owner within the window.
    fn seen(&mut self, owner: u64, error: &str, now: Instant) -> bool {
        self.recent.retain(|(_, _, at)| now.saturating_duration_since(*at) < Self::WINDOW);
        if self.recent.iter().any(|(o, e, _)| *o == owner && e == error) {
            return true;
        }
        if self.recent.len() >= 32 {
            self.recent.remove(0);
        }
        self.recent.push((owner, error.to_string(), now));
        false
    }
}

pub struct Cx {
    pub start: Instant,
    /// Logical time of what is being processed (the arrival time of the current message, or the
    /// deadline of the current timer). Never goes backwards.
    pub now: Instant,
    pub settings: Settings,
    pub timers: Timers<TimerKind>,
    pub out: Output,
    /// Where MIDI goes ("Send MIDI", feedback).
    pub midi: Box<dyn MidiSink>,
    pub exec: Executor,
    pub typematic: Option<Typematic>,
    pub layers: Layers,
    pub gate: Gate,
    pub devices: Devices,
    pub foreground: Option<Foreground>,
    /// Events waiting to be delivered to the sink (in order).
    pub outbox: Vec<EngineEvent>,
    pub requests: Vec<Request>,
    /// Something shown in the status snapshot changed.
    pub dirty: bool,
    pub launcher: Launcher,
    /// Names of the selectable (not always-on) profiles, in order (`ProfileTarget::ByValue`).
    pub selectable: Vec<String>,
    failures: FailureLog,
    /// Outbox index of the `Fired` event whose output is being sent right now: a backend
    /// failure turns that event's outcome into `Failed` instead of adding another event.
    firing: Option<usize>,
}

impl Cx {
    pub fn new(
        start: Instant,
        settings: Settings,
        out: Output,
        midi: Box<dyn MidiSink>,
        launcher: Launcher,
    ) -> Self {
        Cx {
            start,
            now: start,
            gate: Gate { suspend_while_focused: settings.suspend_while_focused, ..Gate::default() },
            settings,
            timers: Timers::new(),
            out,
            midi,
            exec: Executor::default(),
            typematic: None,
            layers: Layers::default(),
            devices: Devices::default(),
            foreground: None,
            outbox: Vec::new(),
            requests: Vec::new(),
            dirty: true,
            launcher,
            selectable: Vec::new(),
            failures: FailureLog::default(),
            firing: None,
        }
    }

    pub fn time_us(&self) -> u64 {
        self.now.saturating_duration_since(self.start).as_micros() as u64
    }

    /// The base for output timing: real time, but never before the logical time. Durations of
    /// what is sent (tap length, gaps) are measured from when it was actually sent.
    pub fn actual_now(&self) -> Instant {
        Instant::now().max(self.now)
    }

    pub fn ms(ms: u32) -> Duration {
        Duration::from_millis(u64::from(ms))
    }

    // ----- reporting -----

    pub fn report(&mut self, info: &OwnerInfo, description: String, outcome: Outcome) {
        self.outbox.push(EngineEvent::Fired(FiredLog {
            time_us: self.time_us(),
            profile_id: info.profile_id.to_string(),
            mapping_id: info.mapping_id.to_string(),
            description,
            outcome,
        }));
    }

    /// `detail`: why, in the user's terms (empty when the reason says it all).
    pub fn report_skip(
        &mut self,
        info: &OwnerInfo,
        description: String,
        reason: SkipReason,
        detail: String,
    ) {
        self.report(info, description, Outcome::Skipped { reason, detail });
    }

    /// Reports a backend failure: as the outcome of the firing being sent right now, or as its
    /// own event unless the same failure was just reported for this owner.
    pub fn report_failure(&mut self, info: &OwnerInfo, description: String, error: OutputError) {
        self.report_failure_message(info, description, error.to_string());
    }

    /// [`Cx::report_failure`] with the error as text.
    pub fn report_failure_message(&mut self, info: &OwnerInfo, description: String, error: String) {
        if let Some(index) = self.firing
            && let Some(EngineEvent::Fired(fired)) = self.outbox.get_mut(index)
        {
            if fired.outcome == Outcome::Sent {
                fired.outcome = Outcome::Failed { error };
            }
            return;
        }
        if self.failures.seen(info.owner, &error, self.now) {
            return;
        }
        self.report(info, description, Outcome::Failed { error });
    }

    /// Reports a firing as sent and runs `send`, which performs its immediate output; a
    /// backend failure during `send` becomes the firing's outcome.
    fn report_sent<R>(
        &mut self,
        info: &OwnerInfo,
        description: String,
        send: impl FnOnce(&mut Cx) -> R,
    ) -> R {
        let index = self.outbox.len();
        self.report(info, description, Outcome::Sent);
        let outer = self.firing.replace(index);
        let result = send(self);
        self.firing = outer;
        result
    }

    pub fn report_failures(&mut self, info: &OwnerInfo, what: &str, errors: Vec<OutputError>) {
        for error in errors {
            self.report_failure(info, what.to_string(), error);
        }
    }

    pub fn error(&mut self, message: String) {
        self.outbox.push(EngineEvent::Error { message });
    }

    // ----- conditions -----

    fn condition_holds(&self, condition: &CCondition) -> bool {
        match condition {
            CCondition::App { rule, negate, .. } => {
                self.foreground.as_ref().is_some_and(|app| rule.matches(app)) != *negate
            }
            CCondition::Layer { layer, active } => self.layers.is_active(layer) == *active,
            CCondition::NoteHeld { channel, note, held } => {
                self.devices.note_held(*channel, *note) == *held
            }
            CCondition::ControlValue { channel, controller, range } => {
                self.devices.control_in(*channel, *controller, *range)
            }
        }
    }

    /// The first condition that does not hold, described for the user (`layer "shift" is
    /// off`, `C2 is not held`, `CC 1 is 12, needs 64-127`, `Photoshop.exe is not in front
    /// (chrome.exe is)`); `None` when all hold.
    pub fn failed_condition(&self, conditions: &[CCondition]) -> Option<String> {
        let failed = conditions.iter().find(|c| !self.condition_holds(c))?;
        let on_channel = |channel: &Option<u8>| match channel {
            Some(channel) => format!(" on channel {channel}"),
            None => String::new(),
        };
        Some(match failed {
            CCondition::Layer { layer, active: true } => format!("layer \"{layer}\" is off"),
            CCondition::Layer { layer, active: false } => format!("layer \"{layer}\" is on"),
            CCondition::NoteHeld { channel, note, held } => {
                let name = m2k_midi::names::note_name(*note, self.settings.middle_c_octave);
                let state = if *held { "is not held" } else { "is held" };
                format!("{name}{} {state}", on_channel(channel))
            }
            CCondition::ControlValue { channel, controller, range } => {
                let which = format!("CC {controller}{}", on_channel(channel));
                match self.devices.control_value(*channel, *controller) {
                    Some(value) => {
                        format!("{which} is {value}, needs {}\u{2013}{}", range.min, range.max)
                    }
                    None => format!("{which} has not been received yet"),
                }
            }
            CCondition::App { negate, process, title, .. } => {
                let wanted = if !process.is_empty() {
                    process.clone()
                } else if !title.is_empty() {
                    format!("a window titled \"{title}\"")
                } else {
                    "an app".to_string()
                };
                let current = self.foreground.as_ref().map(|f| {
                    if f.app.process.is_empty() {
                        f.app.title.clone()
                    } else {
                        f.app.process.clone()
                    }
                });
                match (negate, current) {
                    (true, _) => format!("{wanted} is in front"),
                    (false, Some(current)) if !current.is_empty() => {
                        format!("{wanted} is not in front ({current} is)")
                    }
                    (false, _) => format!("{wanted} is not in front (the app in front is unknown)"),
                }
            }
        })
    }

    // ----- actions -----

    /// Runs an action for `info` (OS output goes through its action queue). With `gated`, the
    /// pause / focus / learning state is respected. Reports the firing. Returns `false` when
    /// the action was skipped.
    pub fn execute(&mut self, info: &OwnerInfo, action: &CAction, ctx: Ctx, gated: bool) -> bool {
        use super::compile::ActionKind;

        let description = action.describe(&ctx, self.settings.middle_c_octave);
        if gated && let Some(reason) = self.gate.check(action.class) {
            self.report_skip(info, description, reason, String::new());
            return false;
        }
        match &action.kind {
            ActionKind::Output(_) => {
                if self.exec.is_full(info.owner) {
                    if self.exec.note_overflow(info.owner, self.now) {
                        let detail = "its action queue is full".to_string();
                        self.report_skip(info, description, SkipReason::RateLimited, detail);
                    }
                    return false;
                }
                let queued =
                    self.report_sent(info, description, |cx| cx.enqueue(info, action, ctx));
                debug_assert!(matches!(queued, Enqueued::Queued));
            }
            ActionKind::Layer { layer, op } => {
                if self.layers.apply(layer, *op) {
                    self.dirty = true;
                }
                self.report(info, description, Outcome::Sent);
            }
            ActionKind::SwitchProfile(ProfileTarget::ByValue) => {
                // The triggering value picks the profile: 0 = the first selectable one.
                let chosen = usize::try_from(ctx.value).ok().filter(|i| *i < self.selectable.len());
                let Some(index) = chosen else {
                    let error = format!(
                        "value {} picks no profile (there are {} to choose from)",
                        ctx.value,
                        self.selectable.len()
                    );
                    self.report(info, description, Outcome::Failed { error });
                    return false;
                };
                let description = format!("switch to profile \"{}\"", self.selectable[index]);
                self.requests.push(Request::SwitchProfile(
                    ProfileTarget::ByValue,
                    info.clone(),
                    Some(index),
                ));
                self.report(info, description, Outcome::Sent);
            }
            ActionKind::SwitchProfile(target) => {
                self.requests.push(Request::SwitchProfile(target.clone(), info.clone(), None));
                self.report(info, description, Outcome::Sent);
            }
            ActionKind::Unavailable(error) => {
                self.report(info, description, Outcome::Failed { error: error.clone() });
                return false;
            }
            ActionKind::Engine(op) => {
                self.requests.push(Request::Engine(*op));
                self.report(info, description, Outcome::Sent);
            }
            ActionKind::Launch(launch) => {
                // Reported by the launcher once the program started (or failed to).
                self.launcher.launch(launch.clone(), info.clone(), description);
            }
            ActionKind::Midi(midi) => {
                let incoming = ctx.value7;
                match self.midi_port(midi.device.as_ref(), &ctx) {
                    Ok(port) => {
                        let octave = self.settings.middle_c_octave;
                        let description = midi.describe(Some(&port), incoming, octave);
                        match midi.send.send(self.midi.as_ref(), &port, incoming) {
                            Ok(()) => self.report(info, description, Outcome::Sent),
                            Err(error) => self.report_failed_once(info, description, error),
                        }
                    }
                    Err(error) => self.report_failed_once(info, description, error),
                }
            }
        }
        true
    }

    // ----- MIDI output -----

    /// The output port for a send: the configured one, or the one named like the device the
    /// triggering message came from (back to the controller itself).
    pub fn midi_port(&self, device: Option<&Arc<str>>, ctx: &Ctx) -> Result<Arc<str>, String> {
        let port = match device {
            Some(name) => name.clone(),
            None => match self.devices.name(ctx.device) {
                Some(input) => match self.midi.paired_output(input) {
                    Some(output) if *output == **input => input.clone(),
                    Some(output) => Arc::from(output),
                    None => {
                        return Err(self.midi.unavailable(input).unwrap_or_else(|| {
                            format!("there is no MIDI output named \"{input}\"")
                        }));
                    }
                },
                None => return Err("choose an output (no message came from a device)".into()),
            },
        };
        // Missing, or switched off: nothing is sent.
        match self.midi.unavailable(&port) {
            None => Ok(port),
            Some(problem) => Err(problem),
        }
    }

    /// A "Send MIDI" macro step (its failures are reported like the macro's other ones).
    pub fn send_midi_step(&mut self, info: &OwnerInfo, midi: &CMidi, ctx: &Ctx) {
        let incoming = ctx.value7;
        let sent = self
            .midi_port(midi.device.as_ref(), ctx)
            .and_then(|port| midi.send.send(self.midi.as_ref(), &port, incoming));
        if let Err(error) = sent {
            let description = midi.describe(None, incoming, self.settings.middle_c_octave);
            self.report_failure_message(info, description, error);
        }
    }

    /// A failed firing, reported once per burst (the same failure of the same owner within a
    /// couple of seconds is not reported again).
    pub fn report_failed_once(&mut self, info: &OwnerInfo, description: String, error: String) {
        if !self.failures.seen(info.owner, &error, self.now) {
            self.report(info, description, Outcome::Failed { error });
        }
    }

    // ----- hold targets -----

    /// Engages a hold target (keys down in order, button down, layer on) unless gating forbids
    /// it. Conditions are the caller's business. Returns whether it was engaged.
    pub fn engage(&mut self, info: &OwnerInfo, target: &CHoldTarget, auto_repeat: bool) -> bool {
        if let Some(reason) = self.gate.check(target.class()) {
            self.report_skip(info, target.hold_label.clone(), reason, String::new());
            return false;
        }
        self.report_sent(info, target.hold_label.clone(), |cx| match &target.what {
            HoldWhat::Keys(keys) => {
                let emulate = auto_repeat && !cx.out.repeats_held_keys();
                cx.hold_keys(info, keys, emulate);
            }
            HoldWhat::Button(button) => cx.press_button(info, *button),
            HoldWhat::Layer(layer) => {
                cx.layers.hold(layer);
            }
        });
        self.dirty = true;
        true
    }

    /// Disengages a hold target. Always goes through. `immediate` releases all keys at once
    /// (forced releases); otherwise keys come up in reverse order with the key stagger.
    pub fn disengage(&mut self, info: &OwnerInfo, target: &CHoldTarget, immediate: bool) {
        self.report_sent(info, target.release_label.clone(), |cx| match &target.what {
            HoldWhat::Keys(_) => cx.unhold_keys(info.owner, immediate),
            HoldWhat::Button(button) => cx.release_button(info, *button),
            HoldWhat::Layer(layer) => {
                cx.layers.unhold(layer);
            }
        });
        self.dirty = true;
    }

    pub fn press_button(&mut self, info: &OwnerInfo, button: MouseButton) {
        self.typematic_cancel();
        if let Err(error) = self.out.press_button(button, info.owner) {
            let what = format!("press {} button", super::compile::button_name(button));
            self.report_failure(info, what, error);
        }
    }

    pub fn release_button(&mut self, info: &OwnerInfo, button: MouseButton) {
        if let Err(error) = self.out.release_button(button, info.owner) {
            let what = format!("release {} button", super::compile::button_name(button));
            self.report_failure(info, what, error);
        }
    }

    /// Releases every key and button still counted for `owner` (safety net after its own
    /// bookkeeping released what it knew about).
    pub fn release_owner(&mut self, info: &OwnerInfo) {
        let errors = self.out.release_owner(info.owner);
        self.report_failures(info, "release", errors);
    }

    /// Schedules a timer `ms` after the logical now.
    pub fn after(&mut self, ms: u32, kind: TimerKind) -> TimerId {
        let at = self.now + Self::ms(ms);
        self.timers.schedule(at, kind)
    }
}
