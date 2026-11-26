//! The shared mutable context of the engine thread ([`Cx`]): timers, output, action queues,
//! layers, gating and the outgoing event buffer. Behaviors receive it next to their own mapping
//! state; everything that has to restructure the engine (profile switches, pause) is queued as a
//! [`Request`] and applied by the runtime once the current message or timer is done.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use m2k_output::{MouseButton, OutputError};

use super::compile::{CAction, CCondition, CHoldTarget, Class, HoldWhat};
use super::executor::{Enqueued, Executor, Typematic};
use super::launch::Launcher;
use super::output::Output;
use super::pattern::Foreground;
use super::source::{DeviceId, Devices};
use super::timers::{TimerId, Timers};
use crate::events::{EngineEvent, FiredLog, Outcome, SkipReason};
use crate::model::{EngineOp, LayerOp, ProfileTarget, Settings};

/// Data of the message that triggered something (for text placeholders and the monitor).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ctx {
    pub device: DeviceId,
    /// 1..=16, 0 for system messages.
    pub channel: u8,
    pub note: Option<u8>,
    pub velocity: Option<u8>,
    /// CC number, 14-bit CC MSB number or NRPN/RPN parameter number.
    pub controller: Option<u16>,
    /// The value (velocity, CC value, 14-bit value, program, signed ticks, ...).
    pub value: i32,
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
    Limiter(u64, super::limiter::Slot),
    /// Key stagger of a hold engagement (owner key).
    Hold(u64),
    /// Next step of an owner's action queue.
    Queue(u64),
    /// Emulated keyboard auto-repeat.
    Typematic,
    /// A delayed `run_action`.
    RunAction(u64),
    /// Coalesced status emission.
    Status,
}

/// Engine-wide changes requested by actions, applied after the current event.
#[derive(Debug, Clone)]
pub enum Request {
    SwitchProfile(ProfileTarget, OwnerInfo),
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
    failures: FailureLog,
}

impl Cx {
    pub fn new(start: Instant, settings: Settings, out: Output, launcher: Launcher) -> Self {
        Cx {
            start,
            now: start,
            gate: Gate { suspend_while_focused: settings.suspend_while_focused, ..Gate::default() },
            settings,
            timers: Timers::new(),
            out,
            exec: Executor::default(),
            typematic: None,
            layers: Layers::default(),
            devices: Devices::default(),
            foreground: None,
            outbox: Vec::new(),
            requests: Vec::new(),
            dirty: true,
            launcher,
            failures: FailureLog::default(),
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

    pub fn report_skip(&mut self, info: &OwnerInfo, description: String, reason: SkipReason) {
        self.report(info, description, Outcome::Skipped { reason });
    }

    /// Reports a backend failure, unless the same failure was just reported for this owner.
    pub fn report_failure(&mut self, info: &OwnerInfo, description: String, error: OutputError) {
        let error = error.to_string();
        if self.failures.seen(info.owner, &error, self.now) {
            return;
        }
        self.report(info, description, Outcome::Failed { error });
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

    pub fn conditions_hold(&self, conditions: &[CCondition]) -> bool {
        conditions.iter().all(|condition| match condition {
            CCondition::App { rule, negate } => {
                self.foreground.as_ref().is_some_and(|app| rule.matches(app)) != *negate
            }
            CCondition::Layer { layer, active } => self.layers.is_active(layer) == *active,
            CCondition::NoteHeld { channel, note, held } => {
                self.devices.note_held(*channel, *note) == *held
            }
            CCondition::ControlValue { channel, controller, range } => {
                self.devices.control_in(*channel, *controller, *range)
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
            self.report_skip(info, description, reason);
            return false;
        }
        match &action.kind {
            ActionKind::Output(_) => {
                if self.exec.is_full(info.owner) {
                    if self.exec.note_overflow(info.owner, self.now) {
                        self.report_skip(info, description, SkipReason::RateLimited);
                    }
                    return false;
                }
                self.report(info, description, Outcome::Sent);
                let queued = self.enqueue(info, action, ctx);
                debug_assert!(matches!(queued, Enqueued::Queued));
            }
            ActionKind::Layer { layer, op } => {
                if self.layers.apply(layer, *op) {
                    self.dirty = true;
                }
                self.report(info, description, Outcome::Sent);
            }
            ActionKind::SwitchProfile(target) => {
                self.requests.push(Request::SwitchProfile(target.clone(), info.clone()));
                self.report(info, description, Outcome::Sent);
            }
            ActionKind::Engine(op) => {
                self.requests.push(Request::Engine(*op));
                self.report(info, description, Outcome::Sent);
            }
            ActionKind::Launch(launch) => {
                self.report(info, description.clone(), Outcome::Sent);
                self.launcher.launch(launch.clone(), info.clone(), description);
            }
        }
        true
    }

    // ----- hold targets -----

    /// Engages a hold target (keys down in order, button down, layer on) unless gating forbids
    /// it. Conditions are the caller's business. Returns whether it was engaged.
    pub fn engage(&mut self, info: &OwnerInfo, target: &CHoldTarget, auto_repeat: bool) -> bool {
        if let Some(reason) = self.gate.check(target.class()) {
            self.report_skip(info, target.hold_label.clone(), reason);
            return false;
        }
        self.report(info, target.hold_label.clone(), Outcome::Sent);
        match &target.what {
            HoldWhat::Keys(keys) => {
                let emulate = auto_repeat && !self.out.repeats_held_keys();
                self.hold_keys(info, keys, emulate);
            }
            HoldWhat::Button(button) => self.press_button(info, *button),
            HoldWhat::Layer(layer) => {
                self.layers.hold(layer);
            }
        }
        self.dirty = true;
        true
    }

    /// Disengages a hold target. Always goes through. `immediate` releases all keys at once
    /// (forced releases); otherwise keys come up in reverse order with the key stagger.
    pub fn disengage(&mut self, info: &OwnerInfo, target: &CHoldTarget, immediate: bool) {
        self.report(info, target.release_label.clone(), Outcome::Sent);
        match &target.what {
            HoldWhat::Keys(_) => self.unhold_keys(info.owner, immediate),
            HoldWhat::Button(button) => self.release_button(info, *button),
            HoldWhat::Layer(layer) => {
                self.layers.unhold(layer);
            }
        }
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
