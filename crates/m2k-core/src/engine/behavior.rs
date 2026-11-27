//! Behaviors: how a mapping interprets the stream of edges its source produces.
//!
//! Each live mapping keeps an instance table (one entry per active note / controller / device /
//! channel its source matches). The table turns raw observations into edges:
//!
//! - `Press`: an instance became active; `Repress`: a note retriggered without a note off;
//!   `Repeat`: a ranged control sent the value it already had (a press-only button pressed
//!   again); `Update`: an active continuous control moved; `Release`: an instance became
//!   inactive; `Pulse`: press and release in one instant; `Ticks`: relative movement.
//! - With `debounce_ms`, the first press/release of an instance passes at once and further
//!   ones are held back for that long; then the table catches up with the control's actual
//!   state (a quick real release is delayed, never lost).
//!
//! Then the behavior decides what fires. Firings go through conditions (evaluated when
//! something is about to fire or engage; releases always go through), the mapping's rate
//! limiter (Trigger / Cycle / Change / Step), and gating (pause, focus, learning) in
//! [`Cx::execute`] / [`Cx::engage`].

use std::sync::Arc;
use std::time::Instant;

use super::compile::{CAction, CBehavior, CFastStep, CMapping, CSource, Class};
use super::cx::{Ctx, Cx, OwnerInfo, TimerKind};
use super::limiter::{Admission, Limiter, SLOTS, Slot};
use super::source::{DeviceId, InstKey, Obs};
use super::timers::TimerId;
use crate::events::SkipReason;

/// Upper bound on the actions one message can fire through a `Step` with `max_burst: 0`.
const MAX_STEPS_PER_MESSAGE: u32 = 256;

/// A `Step` does not queue more output than can be played within this long: a fast spin
/// should not leave seconds of key presses behind it.
const STEP_BACKLOG_MS: u32 = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Press,
    Repress,
    /// A press-only button pressed again (the same in-range value once more).
    Repeat,
    Update,
    Release,
    Pulse,
    Ticks(i32),
}

impl Edge {
    /// Counts as a press for Trigger / Toggle / Cycle (a retrigger counts, pulses count, every
    /// relative message counts once).
    fn is_press(self) -> bool {
        matches!(self, Edge::Press | Edge::Repress | Edge::Repeat | Edge::Pulse | Edge::Ticks(_))
    }

    /// A press/release change of an instance (what debouncing filters).
    fn is_gate(self) -> bool {
        matches!(self, Edge::Press | Edge::Repress | Edge::Repeat | Edge::Release)
    }
}

pub struct Inst {
    pub key: InstKey,
    pub ctx: Ctx,
}

#[derive(Debug, Clone, Copy, Default)]
struct StepRef {
    last: i32,
    acc: i32,
}

/// Debounce window of one instance.
struct Debounce {
    key: InstKey,
    until: Instant,
    /// The control's actual state while the window holds the reported one: active (with its
    /// latest data) or not.
    actual: Option<Ctx>,
    timer: Option<TimerId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapPhase {
    Idle,
    /// Waiting for `hold_ms` (the timer decides "hold").
    Pending(TimerId),
    /// Decided "hold"; `engaged` = the hold target is engaged.
    Holding {
        engaged: bool,
    },
}

pub enum State {
    Trigger {
        repeat: Option<TimerId>,
        last: Ctx,
    },
    Hold {
        engaged: bool,
    },
    /// `by` = the device whose press latched it (a disconnect unlatches).
    Toggle {
        latched: bool,
        by: Option<DeviceId>,
    },
    Cycle {
        index: usize,
        last_press: Option<Instant>,
    },
    TapHold {
        phase: TapPhase,
        press: Ctx,
    },
    MultiTap {
        /// Taps counted so far.
        count: u8,
        /// Decides the count.
        timer: Option<TimerId>,
        last: Ctx,
        /// With a long-press action: a press is down and `hold_ms` has not passed yet.
        long: Option<TimerId>,
        /// The long-press action fired for the press that is still down.
        held: bool,
    },
    Plain,
}

impl State {
    fn initial(behavior: &CBehavior) -> Self {
        match behavior {
            CBehavior::Trigger { .. } => State::Trigger { repeat: None, last: Ctx::default() },
            CBehavior::Hold { .. } => State::Hold { engaged: false },
            CBehavior::Toggle { .. } => State::Toggle { latched: false, by: None },
            CBehavior::Cycle { .. } => State::Cycle { index: 0, last_press: None },
            CBehavior::TapHold { .. } => {
                State::TapHold { phase: TapPhase::Idle, press: Ctx::default() }
            }
            CBehavior::MultiTap { .. } => State::MultiTap {
                count: 0,
                timer: None,
                last: Ctx::default(),
                long: None,
                held: false,
            },
            CBehavior::Change { .. } | CBehavior::Step { .. } | CBehavior::Inert => State::Plain,
        }
    }
}

/// Where a mapping's feedback goes.
#[derive(Debug, Clone)]
pub enum FbTarget {
    Port(Arc<str>),
    /// There is no such output (reported once).
    Missing(Arc<str>),
}

impl FbTarget {
    pub fn name(&self) -> &Arc<str> {
        match self {
            FbTarget::Port(name) | FbTarget::Missing(name) => name,
        }
    }
}

/// Feedback bookkeeping of a mapping (see `worker::feedback`). Survives a panic reset: it records
/// what the controller shows.
#[derive(Debug, Default)]
pub struct FbState {
    /// The device that last reached the mapping (where feedback goes when nothing else says).
    pub last_device: Option<DeviceId>,
    /// Where feedback goes, as resolved for (outputs generation, last device).
    pub target: Option<(u64, Option<DeviceId>, Option<FbTarget>)>,
    /// The port and the state last sent.
    pub sent: Option<(Arc<str>, bool)>,
    /// Send the current state at the next pass even if it was sent already (a refresh).
    pub force: bool,
}

/// Runtime state of one mapping. Survives a config reload when the mapping is unchanged.
pub struct MappingRt {
    /// Unique for the life of the engine; owner key for output and timers.
    pub key: u64,
    pub info: OwnerInfo,
    /// Belongs to an always-on or the effective profile.
    pub live: bool,
    pub instances: Vec<Inst>,
    step_refs: Vec<(InstKey, StepRef)>,
    debounce: Vec<Debounce>,
    pub state: State,
    limiters: [Limiter; SLOTS],
    /// A swallowed firing that will be fired later by the limiter with a specific action (a
    /// fast step) rather than the slot's usual one.
    deferred: [Option<CAction>; SLOTS],
    limit_reported: Option<Instant>,
    pub fb: FbState,
}

impl MappingRt {
    pub fn new(key: u64, def: &CMapping) -> Self {
        MappingRt {
            key,
            info: OwnerInfo {
                owner: key,
                profile_id: def.profile_id.clone(),
                mapping_id: def.id.clone(),
            },
            live: false,
            instances: Vec::new(),
            step_refs: Vec::new(),
            debounce: Vec::new(),
            state: State::initial(&def.behavior),
            limiters: Default::default(),
            deferred: Default::default(),
            limit_reported: None,
            fb: FbState::default(),
        }
    }

    /// Back to a fresh state (after a panic; everything was released already).
    pub fn reset(&mut self, def: &CMapping) {
        self.instances.clear();
        self.step_refs.clear();
        self.debounce.clear();
        self.state = State::initial(&def.behavior);
        self.limiters = Default::default();
        self.deferred = Default::default();
        self.limit_reported = None;
    }

    /// Latched toggle, engaged hold, or tap-hold holding its target.
    pub fn engaged(&self) -> bool {
        match self.state {
            State::Hold { engaged } => engaged,
            State::Toggle { latched, .. } => latched,
            State::TapHold { phase: TapPhase::Holding { engaged }, .. } => engaged,
            _ => false,
        }
    }

    /// Applies an observation to the instance table (through the debounce filter when the
    /// source has one) and returns the resulting edge.
    pub fn observe(
        &mut self,
        def: &CMapping,
        cx: &mut Cx,
        key: InstKey,
        obs: Obs,
        ctx: Ctx,
    ) -> Option<Edge> {
        let source = &def.source;
        let gate = matches!(obs, Obs::Active | Obs::Inactive | Obs::Level { .. });
        if source.debounce_ms == 0 || !gate {
            return self.apply(source, key, obs, ctx);
        }
        if let Some(pos) = self.debounce.iter().position(|d| d.key == key)
            && cx.now < self.debounce[pos].until
        {
            // Inside the window: follow the control, report nothing but value changes of a
            // control that stays held.
            let actual = raw_state(self.debounce[pos].actual.is_some(), obs).then_some(ctx);
            self.debounce[pos].actual = actual;
            if actual.is_some()
                && matches!(obs, Obs::Level { .. })
                && let Some(inst) = self.instances.iter_mut().find(|i| i.key == key)
                && inst.ctx.value != ctx.value
            {
                inst.ctx = ctx;
                return Some(Edge::Update);
            }
            return None;
        }
        let edge = self.apply(source, key, obs, ctx)?;
        if edge.is_gate() {
            self.open_window(cx, source, key);
        }
        Some(edge)
    }

    /// Starts (or restarts) the debounce window of `key` at the reported state.
    fn open_window(&mut self, cx: &mut Cx, source: &CSource, key: InstKey) {
        let until = cx.now + Cx::ms(source.debounce_ms);
        let actual = self.instances.iter().find(|i| i.key == key).map(|i| i.ctx);
        let timer = cx.timers.schedule(until, TimerKind::Debounce(self.key, key));
        match self.debounce.iter_mut().find(|d| d.key == key) {
            Some(window) => {
                cx.timers.cancel_slot(&mut window.timer);
                window.until = until;
                window.actual = actual;
                window.timer = Some(timer);
            }
            None => self.debounce.push(Debounce { key, until, actual, timer: Some(timer) }),
        }
    }

    /// A debounce window ended: if the control's actual state differs from the reported one,
    /// returns the edge that catches up (and opens a new window for it).
    pub fn debounce_expired(
        &mut self,
        def: &CMapping,
        cx: &mut Cx,
        key: InstKey,
        id: TimerId,
    ) -> Option<(Edge, Ctx)> {
        let pos = self.debounce.iter().position(|d| d.key == key && d.timer == Some(id))?;
        let window = self.debounce.swap_remove(pos);
        let reported = self.instances.iter().position(|i| i.key == key);
        let caught_up = match (reported, window.actual) {
            (None, Some(ctx)) => {
                self.instances.push(Inst { key, ctx });
                (Edge::Press, ctx)
            }
            (Some(pos), None) => {
                let ctx = self.instances.swap_remove(pos).ctx;
                (Edge::Release, ctx)
            }
            _ => return None,
        };
        self.open_window(cx, &def.source, key);
        Some(caught_up)
    }

    /// The instance table without debouncing.
    fn apply(&mut self, source: &CSource, key: InstKey, obs: Obs, ctx: Ctx) -> Option<Edge> {
        match obs {
            Obs::Active => match self.instances.iter_mut().find(|i| i.key == key) {
                Some(inst) => {
                    inst.ctx = ctx;
                    Some(if source.retriggers() { Edge::Repress } else { Edge::Update })
                }
                None => {
                    self.instances.push(Inst { key, ctx });
                    Some(Edge::Press)
                }
            },
            Obs::Inactive => {
                let pos = self.instances.iter().position(|i| i.key == key)?;
                self.instances.swap_remove(pos);
                Some(Edge::Release)
            }
            Obs::Level { inside, band } => match self.instances.iter().position(|i| i.key == key) {
                Some(pos) if band => {
                    let repeated = source.presses_on_repeat
                        && inside
                        && self.instances[pos].ctx.value == ctx.value;
                    self.instances[pos].ctx = ctx;
                    Some(if repeated { Edge::Repeat } else { Edge::Update })
                }
                Some(pos) => {
                    self.instances.swap_remove(pos);
                    Some(Edge::Release)
                }
                None if inside => {
                    self.instances.push(Inst { key, ctx });
                    Some(Edge::Press)
                }
                None => None,
            },
            Obs::Pulse => Some(Edge::Pulse),
            Obs::Ticks(n) => Some(Edge::Ticks(n)),
        }
    }

    /// Active instances matching `filter` (with their last trigger data).
    pub fn active(&self, filter: impl Fn(InstKey) -> bool) -> Vec<(InstKey, Ctx)> {
        self.instances.iter().filter(|i| filter(i.key)).map(|i| (i.key, i.ctx)).collect()
    }

    // ----- Step references -----

    fn step_ref(&mut self, key: InstKey) -> Option<&mut StepRef> {
        self.step_refs.iter_mut().find(|(k, _)| *k == key).map(|(_, r)| r)
    }

    /// The reference value of an instance (Step position, Change jitter guard).
    fn reference(&self, key: InstKey) -> Option<i32> {
        self.step_refs.iter().find(|(k, _)| *k == key).map(|(_, r)| r.last)
    }

    fn set_ref(&mut self, key: InstKey, value: i32) {
        match self.step_ref(key) {
            Some(r) => *r = StepRef { last: value, acc: 0 },
            None => self.step_refs.push((key, StepRef { last: value, acc: 0 })),
        }
    }

    /// Absolute movement to `value`; returns whole steps moved (signed). The first value only
    /// sets the reference.
    fn move_ref(&mut self, key: InstKey, value: i32, step: u16) -> i32 {
        match self.step_ref(key) {
            Some(r) => {
                r.acc += value - r.last;
                r.last = value;
                take_steps(&mut r.acc, step)
            }
            None => {
                self.step_refs.push((key, StepRef { last: value, acc: 0 }));
                0
            }
        }
    }

    fn add_ticks(&mut self, key: InstKey, ticks: i32, step: u16) -> i32 {
        if self.step_ref(key).is_none() {
            self.step_refs.push((key, StepRef::default()));
        }
        let r = self.step_ref(key).expect("just inserted");
        r.acc += ticks;
        take_steps(&mut r.acc, step)
    }

    fn drop_ref(&mut self, key: InstKey) {
        self.step_refs.retain(|(k, _)| *k != key);
    }

    /// Forgets debounce windows (of one device, or all).
    fn drop_windows(&mut self, cx: &mut Cx, device: Option<DeviceId>) {
        self.debounce.retain_mut(|window| {
            let drop = device.is_none_or(|d| window.key.device() == d);
            if drop {
                cx.timers.cancel_slot(&mut window.timer);
            }
            !drop
        });
    }
}

/// Whether the control is active after `obs`, given whether it was.
fn raw_state(was_active: bool, obs: Obs) -> bool {
    match obs {
        Obs::Active => true,
        Obs::Inactive => false,
        Obs::Level { inside, band } => {
            if was_active {
                band
            } else {
                inside
            }
        }
        Obs::Pulse | Obs::Ticks(_) => was_active,
    }
}

/// Whole steps in `acc` (toward zero); the remainder stays.
fn take_steps(acc: &mut i32, step: u16) -> i32 {
    let step = i32::from(step.max(1));
    let n = *acc / step;
    *acc -= n * step;
    n
}

// ---------------------------------------------------------------------------------------------
// Firing
// ---------------------------------------------------------------------------------------------

/// The action behind a rate-limitable slot, as of now (a cycle's next action).
fn slot_action(def: &CMapping, state: &State, slot: Slot) -> Option<CAction> {
    match (&def.behavior, slot) {
        (CBehavior::Trigger { on_press, .. }, Slot::Press) => on_press.clone(),
        (CBehavior::Trigger { on_release, .. }, Slot::Release) => on_release.clone(),
        (CBehavior::Cycle { actions, .. }, Slot::Cycle) => match state {
            State::Cycle { index, .. } => actions.get(*index).or(actions.first()).cloned(),
            _ => None,
        },
        (CBehavior::Change { action, .. }, Slot::Change) => Some(action.clone()),
        (CBehavior::Step { up, .. }, Slot::Up) => up.clone(),
        (CBehavior::Step { down, .. }, Slot::Down) => down.clone(),
        _ => None,
    }
}

/// A rate-limitable firing: conditions, then the slot's limiter, then execution. `quiet`
/// suppresses skip reports (auto-fire repeats would flood the monitor).
fn fire(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, slot: Slot, ctx: Ctx, quiet: bool) {
    if let Some(action) = slot_action(def, &rt.state, slot) {
        fire_action(rt, def, cx, slot, action, false, ctx, quiet);
    }
}

/// [`fire`] with an explicit action. `special`: the action is not the slot's usual one (a fast
/// step), so a deferred firing remembers it.
#[allow(clippy::too_many_arguments)]
fn fire_action(
    rt: &mut MappingRt,
    def: &CMapping,
    cx: &mut Cx,
    slot: Slot,
    action: CAction,
    special: bool,
    ctx: Ctx,
    quiet: bool,
) {
    if let Some(detail) = cx.failed_condition(&def.conditions) {
        if !quiet {
            let description = cx.describe(&action, &ctx);
            cx.report_skip(&rt.info, description, SkipReason::ConditionFailed, detail);
        }
        return;
    }
    if let Some(limit) = &def.rate_limit {
        let limiter = &mut rt.limiters[slot.index()];
        match limiter.admit(limit, cx.now, ctx) {
            Admission::Fire => {}
            Admission::Defer(at) => {
                if limiter.timer.is_none() {
                    limiter.timer = Some(cx.timers.schedule(at, TimerKind::Limiter(rt.key, slot)));
                }
                let detail = limiter.explain(limit, cx.now);
                // The latest trigger wins, with its own action.
                rt.deferred[slot.index()] = special.then(|| action.clone());
                note_rate_limited(rt, cx, &action, &ctx, limit.interval_ms, detail);
                return;
            }
            Admission::Drop => {
                let detail = limiter.explain(limit, cx.now);
                note_rate_limited(rt, cx, &action, &ctx, limit.interval_ms, detail);
                return;
            }
        }
    }
    run_slot(rt, def, cx, slot, &action, ctx, quiet);
}

fn run_slot(
    rt: &mut MappingRt,
    def: &CMapping,
    cx: &mut Cx,
    slot: Slot,
    action: &CAction,
    ctx: Ctx,
    quiet: bool,
) {
    if quiet && cx.gate.check(action.class).is_some() {
        return;
    }
    let executed = cx.execute(&rt.info, action, ctx, true);
    if executed
        && slot == Slot::Cycle
        && let (CBehavior::Cycle { actions, .. }, State::Cycle { index, .. }) =
            (&def.behavior, &mut rt.state)
        && !actions.is_empty()
    {
        *index = (*index + 1) % actions.len();
    }
}

/// At most one "rate limited" report per mapping per interval.
fn note_rate_limited(
    rt: &mut MappingRt,
    cx: &mut Cx,
    action: &CAction,
    ctx: &Ctx,
    interval_ms: u32,
    detail: String,
) {
    let window = Cx::ms(interval_ms.max(1));
    if rt.limit_reported.is_some_and(|at| cx.now.saturating_duration_since(at) < window) {
        return;
    }
    rt.limit_reported = Some(cx.now);
    let description = cx.describe(action, ctx);
    cx.report_skip(&rt.info, description, SkipReason::RateLimited, detail);
}

/// A firing that is not rate limited (tap-hold, multi-tap, hold pulses).
fn fire_direct(rt: &MappingRt, def: &CMapping, cx: &mut Cx, action: &CAction, ctx: Ctx) {
    if let Some(detail) = cx.failed_condition(&def.conditions) {
        let description = cx.describe(action, &ctx);
        cx.report_skip(&rt.info, description, SkipReason::ConditionFailed, detail);
        return;
    }
    cx.execute(&rt.info, action, ctx, true);
}

/// The Step backlog is full when the queued output would take longer than this to play.
fn step_backlog_full(rt: &MappingRt, cx: &Cx, action: &CAction) -> Option<String> {
    let backlog = cx.exec.backlog_ms(rt.key, cx.now);
    (action.queues() && backlog > STEP_BACKLOG_MS)
        .then(|| format!("{backlog} ms of steps are already queued"))
}

// ---------------------------------------------------------------------------------------------
// Edges
// ---------------------------------------------------------------------------------------------

/// Feeds one edge of instance `inst` to the mapping's behavior. `was_active` = the mapping had
/// at least one active instance before this edge.
pub fn on_edge(
    rt: &mut MappingRt,
    def: &CMapping,
    cx: &mut Cx,
    inst: InstKey,
    edge: Edge,
    ctx: Ctx,
    was_active: bool,
) {
    // A repeated value on the only active instance is a release and a new press of the
    // mapping as a whole.
    let only = rt.instances.len() == 1;
    match &def.behavior {
        CBehavior::Trigger { on_press, repeat, .. } => match edge {
            Edge::Press | Edge::Repress | Edge::Repeat => {
                if edge == Edge::Repeat {
                    fire(rt, def, cx, Slot::Release, ctx, false);
                }
                if let State::Trigger { last, .. } = &mut rt.state {
                    *last = ctx;
                }
                fire(rt, def, cx, Slot::Press, ctx, false);
                if let (Some(repeat), Some(_)) = (repeat, on_press)
                    && let State::Trigger { repeat: timer, .. } = &mut rt.state
                {
                    // A new press restarts the delay.
                    cx.timers.cancel_slot(timer);
                    *timer = Some(cx.after(repeat.delay_ms, TimerKind::Repeat(rt.key)));
                }
            }
            Edge::Release => {
                fire(rt, def, cx, Slot::Release, ctx, false);
                if rt.instances.is_empty()
                    && let State::Trigger { repeat, .. } = &mut rt.state
                {
                    cx.timers.cancel_slot(repeat);
                }
            }
            Edge::Pulse | Edge::Ticks(_) => {
                fire(rt, def, cx, Slot::Press, ctx, false);
                fire(rt, def, cx, Slot::Release, ctx, false);
            }
            Edge::Update => {}
        },

        CBehavior::Hold { target, auto_repeat, pulse } => match edge {
            Edge::Press => {
                if rt.engaged() {
                    return;
                }
                if let Some(detail) = cx.failed_condition(&def.conditions) {
                    let label = cx.label(&target.hold_label);
                    cx.report_skip(&rt.info, label, SkipReason::ConditionFailed, detail);
                    return;
                }
                if cx.engage(&rt.info, target, *auto_repeat) {
                    rt.state = State::Hold { engaged: true };
                }
            }
            Edge::Release => {
                if rt.instances.is_empty() && rt.engaged() {
                    cx.disengage(&rt.info, target, false);
                    rt.state = State::Hold { engaged: false };
                }
            }
            Edge::Pulse | Edge::Ticks(_) => {
                if !rt.engaged()
                    && let Some(tap) = pulse
                {
                    fire_direct(rt, def, cx, tap, ctx);
                }
            }
            // Held stays held.
            Edge::Repress | Edge::Repeat | Edge::Update => {}
        },

        CBehavior::Toggle { target } => {
            if !edge.is_press() {
                return;
            }
            if rt.engaged() {
                cx.disengage(&rt.info, target, false);
                rt.state = State::Toggle { latched: false, by: None };
            } else if let Some(detail) = cx.failed_condition(&def.conditions) {
                let label = cx.label(&target.hold_label);
                cx.report_skip(&rt.info, label, SkipReason::ConditionFailed, detail);
            } else if cx.engage(&rt.info, target, false) {
                rt.state = State::Toggle { latched: true, by: Some(ctx.device) };
            }
        }

        CBehavior::Cycle { actions, reset_after_ms } => {
            if !edge.is_press() || actions.is_empty() {
                return;
            }
            if let State::Cycle { index, last_press } = &mut rt.state {
                if let (Some(reset), Some(last)) = (reset_after_ms, *last_press)
                    && cx.now.saturating_duration_since(last) >= Cx::ms(*reset)
                {
                    *index = 0;
                }
                *last_press = Some(cx.now);
            }
            fire(rt, def, cx, Slot::Cycle, ctx, false);
        }

        CBehavior::TapHold { tap, .. } => {
            let active = !rt.instances.is_empty();
            match edge {
                Edge::Press if !was_active => tap_hold_press(rt, def, cx, ctx),
                Edge::Release if !active => tap_hold_release(rt, def, cx),
                Edge::Repeat if only => {
                    tap_hold_release(rt, def, cx);
                    tap_hold_press(rt, def, cx, ctx);
                }
                Edge::Pulse | Edge::Ticks(_) => {
                    let idle = matches!(rt.state, State::TapHold { phase: TapPhase::Idle, .. });
                    if !active
                        && idle
                        && let Some(tap) = tap
                    {
                        fire_direct(rt, def, cx, tap, ctx);
                    }
                }
                _ => {}
            }
        }

        CBehavior::MultiTap { .. } => match edge {
            Edge::Press if !was_active => multi_tap_press(rt, def, cx, ctx),
            Edge::Repress | Edge::Repeat => {
                if only {
                    multi_tap_release(rt, def, cx);
                }
                multi_tap_press(rt, def, cx, ctx);
            }
            Edge::Release if rt.instances.is_empty() => multi_tap_release(rt, def, cx),
            Edge::Pulse | Edge::Ticks(_) => {
                multi_tap_press(rt, def, cx, ctx);
                multi_tap_release(rt, def, cx);
            }
            _ => {}
        },

        CBehavior::Change { min_change, .. } => {
            if edge == Edge::Release {
                return;
            }
            // Jitter guard: a value counts only once it is `min_change` away from the last one
            // that counted (the first always counts). The reference moves before the rate
            // limiter decides; a value that does not count reports nothing.
            if *min_change > 1
                && def.source.ranged
                && matches!(edge, Edge::Press | Edge::Update | Edge::Repeat)
            {
                if rt
                    .reference(inst)
                    .is_some_and(|r| (ctx.value - r).unsigned_abs() < u32::from(*min_change))
                {
                    return;
                }
                rt.set_ref(inst, ctx.value);
            }
            fire(rt, def, cx, Slot::Change, ctx, false);
        }

        CBehavior::Inert => {}

        CBehavior::Step { up, down, step, max_burst, invert, fast } => {
            let moved = match edge {
                Edge::Press => {
                    rt.set_ref(inst, ctx.value);
                    0
                }
                Edge::Update | Edge::Repress | Edge::Repeat | Edge::Pulse => {
                    rt.move_ref(inst, ctx.value, *step)
                }
                Edge::Release => {
                    rt.drop_ref(inst);
                    0
                }
                Edge::Ticks(ticks) => rt.add_ticks(inst, ticks, *step),
            };
            if moved == 0 {
                return;
            }
            let up_now = (moved > 0) != *invert;
            let steps = moved.unsigned_abs();
            let (slot, this, other) =
                if up_now { (Slot::Up, up, down) } else { (Slot::Down, down, up) };
            let fast_of =
                |fast: &CFastStep, up: bool| if up { fast.up.clone() } else { fast.down.clone() };
            // Turning back cancels what is still queued for the other direction (fine or fast).
            let other_fast = fast.as_ref().and_then(|fast| fast_of(fast, !up_now));
            cx.exec.drop_pending(rt.key, |queued| {
                other.as_ref().is_some_and(|o| Arc::ptr_eq(queued, o))
                    || other_fast.as_ref().is_some_and(|o| Arc::ptr_eq(queued, o))
            });
            // Moved by `ticks` or more at once: one coarse action instead of many fine ones.
            if let Some(fast_action) = fast
                .as_ref()
                .filter(|fast| steps >= fast.ticks)
                .and_then(|fast| fast_of(fast, up_now))
            {
                if let Some(detail) = step_backlog_full(rt, cx, &fast_action) {
                    note_rate_limited(rt, cx, &fast_action, &ctx, 500, detail);
                    return;
                }
                fire_action(rt, def, cx, slot, fast_action, true, ctx, false);
                return;
            }
            let mut count = steps;
            if *max_burst > 0 {
                count = count.min(u32::from(*max_burst));
            }
            // "Unlimited" still has to end somewhere: a 14-bit jump with step 1 would otherwise
            // queue thousands of firings for one message.
            count = count.min(MAX_STEPS_PER_MESSAGE);
            for _ in 0..count {
                if let Some(action) = this
                    && let Some(detail) = step_backlog_full(rt, cx, action)
                {
                    note_rate_limited(rt, cx, action, &ctx, 500, detail);
                    break;
                }
                fire(rt, def, cx, slot, ctx, false);
            }
        }
    }
}

// ----- tap-hold -----

/// The mapping as a whole went down.
fn tap_hold_press(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, ctx: Ctx) {
    let CBehavior::TapHold { hold_target, hold_ms, .. } = &def.behavior else {
        return;
    };
    // A release always ends a hold before the next press; should that ever not be the case,
    // never leak the engaged target.
    if let State::TapHold { phase: TapPhase::Holding { engaged: true }, .. } = rt.state
        && let Some(target) = hold_target
    {
        cx.disengage(&rt.info, target, true);
    }
    let timer = cx.after(*hold_ms, TimerKind::TapHold(rt.key));
    if let State::TapHold { phase, press } = &mut rt.state {
        if let TapPhase::Pending(old) = *phase {
            cx.timers.cancel(old);
        }
        *phase = TapPhase::Pending(timer);
        *press = ctx;
    }
}

/// The mapping as a whole went up.
fn tap_hold_release(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx) {
    let CBehavior::TapHold { tap, hold_target, .. } = &def.behavior else {
        return;
    };
    let State::TapHold { phase, press } = &mut rt.state else {
        return;
    };
    let previous = std::mem::replace(phase, TapPhase::Idle);
    let press = *press;
    match previous {
        TapPhase::Pending(timer) => {
            cx.timers.cancel(timer);
            if let Some(tap) = tap {
                fire_direct(rt, def, cx, tap, press);
            }
        }
        TapPhase::Holding { engaged: true } => {
            if let Some(target) = hold_target {
                cx.disengage(&rt.info, target, false);
            }
        }
        _ => {}
    }
}

// ----- multi-tap -----

/// A press of a multi-tap mapping. Without a long-press action it counts right away; with
/// one, it counts on release (if released before `hold_ms`).
fn multi_tap_press(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, ctx: Ctx) {
    let CBehavior::MultiTap { highest, window_ms, hold, hold_ms, .. } = &def.behavior else {
        return;
    };
    let key = rt.key;
    let decide = {
        let State::MultiTap { count, timer, last, long, held } = &mut rt.state else {
            return;
        };
        *last = ctx;
        if hold.is_some() {
            // The window runs from the last release; wait for this press to end (or last).
            cx.timers.cancel_slot(timer);
            cx.timers.cancel_slot(long);
            *long = Some(cx.after(*hold_ms, TimerKind::LongPress(key)));
            *held = false;
            None
        } else if *highest == 0 {
            None
        } else {
            cx.timers.cancel_slot(timer);
            *count = count.saturating_add(1);
            if *count >= *highest {
                Some(std::mem::take(count))
            } else {
                *timer = Some(cx.after(*window_ms, TimerKind::MultiTap(key)));
                None
            }
        }
    };
    if let Some(n) = decide {
        decide_multi_tap(rt, def, cx, n, ctx);
    }
}

/// A release of a multi-tap mapping (only matters with a long-press action).
fn multi_tap_release(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx) {
    let CBehavior::MultiTap { highest, window_ms, hold: Some(_), .. } = &def.behavior else {
        return;
    };
    let key = rt.key;
    let decide = {
        let State::MultiTap { count, timer, last, long, held } = &mut rt.state else {
            return;
        };
        if std::mem::take(held) {
            // The long press already fired.
            return;
        }
        if long.is_none() {
            // No press in progress.
            return;
        }
        cx.timers.cancel_slot(long);
        if *highest == 0 {
            *count = 0;
            return;
        }
        *count = count.saturating_add(1);
        if *count >= *highest {
            Some((std::mem::take(count), *last))
        } else {
            cx.timers.cancel_slot(timer);
            *timer = Some(cx.after(*window_ms, TimerKind::MultiTap(key)));
            None
        }
    };
    if let Some((n, ctx)) = decide {
        decide_multi_tap(rt, def, cx, n, ctx);
    }
}

fn decide_multi_tap(rt: &MappingRt, def: &CMapping, cx: &mut Cx, n: u8, ctx: Ctx) {
    if let CBehavior::MultiTap { actions, .. } = &def.behavior
        && let Some(Some(action)) = actions.get(usize::from(n.max(1)) - 1)
    {
        fire_direct(rt, def, cx, action, ctx);
    }
}

// ---------------------------------------------------------------------------------------------
// Timers
// ---------------------------------------------------------------------------------------------

pub fn on_timer(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, kind: TimerKind, id: TimerId) {
    match kind {
        TimerKind::Repeat(_) => on_repeat(rt, def, cx, id),
        TimerKind::TapHold(_) => on_tap_hold(rt, def, cx, id),
        TimerKind::MultiTap(_) => {
            let decide = match &mut rt.state {
                State::MultiTap { count, timer, last, .. } if *timer == Some(id) => {
                    *timer = None;
                    let n = std::mem::take(count);
                    Some((n, *last))
                }
                _ => None,
            };
            if let Some((n, ctx)) = decide
                && n > 0
                && rt.live
            {
                decide_multi_tap(rt, def, cx, n, ctx);
            }
        }
        TimerKind::LongPress(_) => on_long_press(rt, def, cx, id),
        TimerKind::Limiter(_, slot) => on_limiter(rt, def, cx, slot, id),
        TimerKind::Debounce(_, inst) => {
            let was_active = !rt.instances.is_empty();
            if rt.live
                && let Some((edge, ctx)) = rt.debounce_expired(def, cx, inst, id)
            {
                on_edge(rt, def, cx, inst, edge, ctx, was_active);
            }
        }
        _ => {}
    }
}

fn on_repeat(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, id: TimerId) {
    let ctx = match &mut rt.state {
        State::Trigger { repeat, last } if *repeat == Some(id) => {
            *repeat = None;
            *last
        }
        _ => return,
    };
    let CBehavior::Trigger { repeat: Some(repeat), on_press: Some(_), .. } = &def.behavior else {
        return;
    };
    if rt.instances.is_empty() || !rt.live {
        return;
    }
    // One repeat at a time: when the previous one has not even started, skip this one, so a
    // released pad does not keep firing from a backlog.
    if !cx.exec.has_pending(rt.key) {
        fire(rt, def, cx, Slot::Press, ctx, true);
    }
    let interval = Cx::ms(repeat.interval_ms.max(1));
    let mut next = id.at() + interval;
    let real = Instant::now();
    if next < real {
        next = real + interval;
    }
    let timer = cx.timers.schedule(next, TimerKind::Repeat(rt.key));
    if let State::Trigger { repeat, .. } = &mut rt.state {
        *repeat = Some(timer);
    }
}

fn on_tap_hold(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, id: TimerId) {
    let ctx = match &mut rt.state {
        State::TapHold { phase, press } if *phase == TapPhase::Pending(id) => {
            *phase = TapPhase::Holding { engaged: false };
            *press
        }
        _ => return,
    };
    let CBehavior::TapHold { hold, hold_target, .. } = &def.behavior else {
        return;
    };
    if !rt.live || rt.instances.is_empty() {
        return;
    }
    if let Some(detail) = cx.failed_condition(&def.conditions) {
        let description = match (hold, hold_target) {
            (Some(action), _) => cx.describe(action, &ctx),
            (None, Some(target)) => cx.label(&target.hold_label),
            (None, None) => return,
        };
        cx.report_skip(&rt.info, description, SkipReason::ConditionFailed, detail);
        return;
    }
    if let Some(action) = hold {
        cx.execute(&rt.info, action, ctx, true);
    }
    if let Some(target) = hold_target
        && cx.engage(&rt.info, target, false)
        && let State::TapHold { phase, .. } = &mut rt.state
    {
        *phase = TapPhase::Holding { engaged: true };
    }
}

/// A multi-tap press lasted `hold_ms`: the long-press action fires once and the taps counted
/// so far are dropped.
fn on_long_press(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, id: TimerId) {
    let ctx = match &mut rt.state {
        State::MultiTap { count, timer, last, long, held } if *long == Some(id) => {
            *long = None;
            *held = true;
            *count = 0;
            cx.timers.cancel_slot(timer);
            *last
        }
        _ => return,
    };
    if let CBehavior::MultiTap { hold: Some(action), .. } = &def.behavior
        && rt.live
        && !rt.instances.is_empty()
    {
        fire_direct(rt, def, cx, action, ctx);
    }
}

fn on_limiter(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, slot: Slot, id: TimerId) {
    let Some(limit) = &def.rate_limit else {
        return;
    };
    let limiter = &mut rt.limiters[slot.index()];
    if limiter.timer != Some(id) {
        return;
    }
    limiter.timer = None;
    let (ctx, again) = limiter.expire(limit, cx.now);
    if let Some(at) = again {
        limiter.timer = Some(cx.timers.schedule(at, TimerKind::Limiter(rt.key, slot)));
    }
    let Some(ctx) = ctx else {
        return;
    };
    if !rt.live {
        return;
    }
    let deferred = rt.deferred[slot.index()].take();
    let Some(action) = deferred.or_else(|| slot_action(def, &rt.state, slot)) else {
        return;
    };
    if let Some(detail) = cx.failed_condition(&def.conditions) {
        let description = cx.describe(&action, &ctx);
        cx.report_skip(&rt.info, description, SkipReason::ConditionFailed, detail);
        return;
    }
    run_slot(rt, def, cx, slot, &action, ctx, false);
}

// ---------------------------------------------------------------------------------------------
// Forced releases
// ---------------------------------------------------------------------------------------------

/// Releases what the mapping holds, immediately: OS output only (`all = false`; pause, focus
/// suspension, learning, backend swap) or everything including layers (`all = true`; release
/// all, deactivation). Toggles unlatch, auto-fire stops. Holds are not re-engaged later.
pub fn release(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, all: bool) {
    match (&def.behavior, &mut rt.state) {
        (CBehavior::Hold { target, .. }, State::Hold { engaged })
            if *engaged && (all || target.class() == Class::Os) =>
        {
            cx.disengage(&rt.info, target, true);
            *engaged = false;
        }
        (CBehavior::Toggle { target }, State::Toggle { latched, by })
            if *latched && (all || target.class() == Class::Os) =>
        {
            cx.disengage(&rt.info, target, true);
            *latched = false;
            *by = None;
        }
        (
            CBehavior::TapHold { hold_target: Some(target), .. },
            State::TapHold { phase: TapPhase::Holding { engaged }, .. },
        ) if *engaged && (all || target.class() == Class::Os) => {
            cx.disengage(&rt.info, target, true);
            *engaged = false;
        }
        (_, State::Trigger { repeat, .. }) => cx.timers.cancel_slot(repeat),
        _ => {}
    }
}

/// The mapping stops being live (profile switch) or is removed / changed (config reload):
/// release what it holds, cancel its timers, forget its instances and pending decisions.
/// Actions it already fired play out (each releases what it pressed when it ends): a macro
/// that brings another app to the front, and so switches profiles, must not be cut short, and
/// what the monitor reported as sent is sent. Removing or changing the mapping aborts them too
/// (`Worker::install`).
pub fn deactivate(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx) {
    release(rt, def, cx, true);
    match &mut rt.state {
        State::Trigger { repeat, .. } => cx.timers.cancel_slot(repeat),
        State::TapHold { phase, .. } => {
            if let TapPhase::Pending(timer) = *phase {
                cx.timers.cancel(timer);
            }
            *phase = TapPhase::Idle;
        }
        State::MultiTap { count, timer, long, held, .. } => {
            cx.timers.cancel_slot(timer);
            cx.timers.cancel_slot(long);
            *count = 0;
            *held = false;
        }
        _ => {}
    }
    for limiter in &mut rt.limiters {
        cx.timers.cancel_slot(&mut limiter.timer);
        limiter.clear();
    }
    rt.deferred = Default::default();
    rt.drop_windows(cx, None);
    rt.instances.clear();
    rt.step_refs.clear();
    rt.limit_reported = None;
    cx.unhold_keys(rt.key, true);
    // Safety net for what its own bookkeeping missed; while an action is still playing, the
    // keys it holds are its own, released when it ends.
    if !cx.exec.busy(rt.key) {
        cx.release_owner(&rt.info);
    }
    cx.dirty = true;
}

/// A device disconnected: forget its instances and Step references; whatever they were holding
/// is released (without firing release actions); toggles latched from that device unlatch.
/// Returns whether the device was holding anything here.
pub fn drop_device(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, device: DeviceId) -> bool {
    let latched_here =
        matches!(rt.state, State::Toggle { latched: true, by: Some(by) } if by == device);
    // Feedback no longer answers a device that is gone.
    if rt.fb.last_device == Some(device) {
        rt.fb.last_device = None;
    }
    let before = rt.instances.len();
    rt.instances.retain(|i| i.key.device() != device);
    rt.step_refs.retain(|(k, _)| k.device() != device);
    rt.drop_windows(cx, Some(device));
    let removed = rt.instances.len() != before;
    let idle = rt.instances.is_empty();
    match (&def.behavior, &mut rt.state) {
        (CBehavior::Hold { target, .. }, State::Hold { engaged })
            if removed && idle && *engaged =>
        {
            cx.disengage(&rt.info, target, true);
            *engaged = false;
        }
        (CBehavior::Trigger { .. }, State::Trigger { repeat, .. }) if removed && idle => {
            cx.timers.cancel_slot(repeat);
        }
        (CBehavior::Toggle { target }, State::Toggle { latched, by })
            if *latched && *by == Some(device) =>
        {
            cx.disengage(&rt.info, target, true);
            *latched = false;
            *by = None;
        }
        (CBehavior::TapHold { hold_target, .. }, State::TapHold { phase, .. })
            if removed && idle =>
        {
            match *phase {
                TapPhase::Pending(timer) => cx.timers.cancel(timer),
                TapPhase::Holding { engaged: true } => {
                    if let Some(target) = hold_target {
                        cx.disengage(&rt.info, target, true);
                    }
                }
                _ => {}
            }
            *phase = TapPhase::Idle;
        }
        (CBehavior::MultiTap { .. }, State::MultiTap { long, held, .. }) if removed && idle => {
            // A press that was in progress ends without counting.
            cx.timers.cancel_slot(long);
            *held = false;
        }
        _ => {}
    }
    removed || latched_here
}
