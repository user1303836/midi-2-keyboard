//! Behaviors: how a mapping interprets the stream of edges its source produces.
//!
//! Each live mapping keeps an instance table (one entry per active note / controller / device /
//! channel its source matches). The table turns raw observations into edges:
//!
//! - `Press`: an instance became active; `Repress`: a note retriggered without a note off;
//!   `Update`: an active continuous control moved inside its range; `Release`: an instance
//!   became inactive; `Pulse`: press and release in one instant; `Ticks`: relative movement.
//!
//! Then the behavior decides what fires. Firings go through conditions (evaluated when
//! something is about to fire or engage; releases always go through), the mapping's rate
//! limiter (Trigger / Cycle / Change / Step), and gating (pause, focus, learning) in
//! [`Cx::execute`] / [`Cx::engage`].

use std::time::Instant;

use super::compile::{CAction, CBehavior, CMapping, Class};
use super::cx::{Ctx, Cx, OwnerInfo, TimerKind};
use super::limiter::{Admission, Limiter, SLOTS, Slot};
use super::source::{DeviceId, InstKey, Obs};
use super::timers::TimerId;
use crate::events::SkipReason;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Press,
    Repress,
    Update,
    Release,
    Pulse,
    Ticks(i32),
}

impl Edge {
    /// Counts as a press for Trigger / Toggle / Cycle (a retrigger counts, pulses count, every
    /// relative message counts once).
    fn is_press(self) -> bool {
        matches!(self, Edge::Press | Edge::Repress | Edge::Pulse | Edge::Ticks(_))
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapPhase {
    Idle,
    /// Waiting for `hold_ms` (the timer decides "hold").
    Pending(TimerId),
    /// Decided "hold"; `engaged` = the hold target is engaged.
    Holding { engaged: bool },
}

pub enum State {
    Trigger { repeat: Option<TimerId>, last: Ctx },
    Hold { engaged: bool },
    /// `by` = the device whose press latched it (a disconnect unlatches).
    Toggle { latched: bool, by: Option<DeviceId> },
    Cycle { index: usize, last_press: Option<Instant> },
    TapHold { phase: TapPhase, press: Ctx },
    MultiTap { count: u8, timer: Option<TimerId>, last: Ctx },
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
            CBehavior::MultiTap { .. } => {
                State::MultiTap { count: 0, timer: None, last: Ctx::default() }
            }
            CBehavior::Change { .. } | CBehavior::Step { .. } => State::Plain,
        }
    }
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
    pub state: State,
    limiters: [Limiter; SLOTS],
    limit_reported: Option<Instant>,
}

impl MappingRt {
    pub fn new(key: u64, def: &CMapping) -> Self {
        MappingRt {
            key,
            info: OwnerInfo { owner: key, profile_id: def.profile_id.clone(), mapping_id: def.id.clone() },
            live: false,
            instances: Vec::new(),
            step_refs: Vec::new(),
            state: State::initial(&def.behavior),
            limiters: Default::default(),
            limit_reported: None,
        }
    }

    /// Back to a fresh state (after a panic; everything was released already).
    pub fn reset(&mut self, def: &CMapping) {
        self.instances.clear();
        self.step_refs.clear();
        self.state = State::initial(&def.behavior);
        self.limiters = Default::default();
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

    /// Applies an observation to the instance table and returns the resulting edge.
    pub fn observe(&mut self, key: InstKey, obs: Obs, retriggers: bool, ctx: Ctx) -> Option<Edge> {
        match obs {
            Obs::Active => match self.instances.iter_mut().find(|i| i.key == key) {
                Some(inst) => {
                    inst.ctx = ctx;
                    Some(if retriggers { Edge::Repress } else { Edge::Update })
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
        (CBehavior::Change { action }, Slot::Change) => Some(action.clone()),
        (CBehavior::Step { up, .. }, Slot::Up) => up.clone(),
        (CBehavior::Step { down, .. }, Slot::Down) => down.clone(),
        _ => None,
    }
}

/// A rate-limitable firing: conditions, then the slot's limiter, then execution. `quiet`
/// suppresses skip reports (auto-fire repeats would flood the monitor).
fn fire(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, slot: Slot, ctx: Ctx, quiet: bool) {
    let Some(action) = slot_action(def, &rt.state, slot) else {
        return;
    };
    if !cx.conditions_hold(&def.conditions) {
        if !quiet {
            let description = action.describe(&ctx, cx.settings.middle_c_octave);
            cx.report_skip(&rt.info, description, SkipReason::ConditionFailed);
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
                note_rate_limited(rt, cx, &action, &ctx, limit.interval_ms);
                return;
            }
            Admission::Drop => {
                note_rate_limited(rt, cx, &action, &ctx, limit.interval_ms);
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
fn note_rate_limited(rt: &mut MappingRt, cx: &mut Cx, action: &CAction, ctx: &Ctx, interval_ms: u32) {
    let window = Cx::ms(interval_ms.max(1));
    if rt.limit_reported.is_some_and(|at| cx.now.saturating_duration_since(at) < window) {
        return;
    }
    rt.limit_reported = Some(cx.now);
    let description = action.describe(ctx, cx.settings.middle_c_octave);
    cx.report_skip(&rt.info, description, SkipReason::RateLimited);
}

/// A firing that is not rate limited (tap-hold, multi-tap, hold pulses).
fn fire_direct(rt: &MappingRt, def: &CMapping, cx: &mut Cx, action: &CAction, ctx: Ctx) {
    if !cx.conditions_hold(&def.conditions) {
        let description = action.describe(&ctx, cx.settings.middle_c_octave);
        cx.report_skip(&rt.info, description, SkipReason::ConditionFailed);
        return;
    }
    cx.execute(&rt.info, action, ctx, true);
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
    match &def.behavior {
        CBehavior::Trigger { on_press, repeat, .. } => match edge {
            Edge::Press | Edge::Repress => {
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
                if !cx.conditions_hold(&def.conditions) {
                    cx.report_skip(&rt.info, target.hold_label.clone(), SkipReason::ConditionFailed);
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
            Edge::Repress | Edge::Update => {}
        },

        CBehavior::Toggle { target } => {
            if !edge.is_press() {
                return;
            }
            if rt.engaged() {
                cx.disengage(&rt.info, target, false);
                rt.state = State::Toggle { latched: false, by: None };
            } else if !cx.conditions_hold(&def.conditions) {
                cx.report_skip(&rt.info, target.hold_label.clone(), SkipReason::ConditionFailed);
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

        CBehavior::TapHold { tap, hold_target, hold_ms, .. } => {
            let active = !rt.instances.is_empty();
            match edge {
                Edge::Press if !was_active => {
                    let timer = cx.after(*hold_ms, TimerKind::TapHold(rt.key));
                    if let State::TapHold { phase, press } = &mut rt.state {
                        if let TapPhase::Pending(old) = *phase {
                            cx.timers.cancel(old);
                        }
                        *phase = TapPhase::Pending(timer);
                        *press = ctx;
                    }
                }
                Edge::Release if !active => {
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

        CBehavior::MultiTap { highest, window_ms, .. } => {
            let press = match edge {
                Edge::Press => !was_active,
                Edge::Repress | Edge::Pulse | Edge::Ticks(_) => true,
                Edge::Update | Edge::Release => false,
            };
            if !press || *highest == 0 {
                return;
            }
            let decide = {
                let State::MultiTap { count, timer, last } = &mut rt.state else {
                    return;
                };
                cx.timers.cancel_slot(timer);
                *count = count.saturating_add(1);
                *last = ctx;
                if *count >= *highest {
                    let n = *count;
                    *count = 0;
                    Some(n)
                } else {
                    *timer = Some(cx.after(*window_ms, TimerKind::MultiTap(rt.key)));
                    None
                }
            };
            if let Some(n) = decide {
                decide_multi_tap(rt, def, cx, n, ctx);
            }
        }

        CBehavior::Change { .. } => {
            if matches!(
                edge,
                Edge::Press | Edge::Repress | Edge::Update | Edge::Pulse | Edge::Ticks(_)
            ) {
                fire(rt, def, cx, Slot::Change, ctx, false);
            }
        }

        CBehavior::Step { step, max_burst, invert, .. } => {
            let moved = match edge {
                Edge::Press => {
                    rt.set_ref(inst, ctx.value);
                    0
                }
                Edge::Update | Edge::Repress | Edge::Pulse => rt.move_ref(inst, ctx.value, *step),
                Edge::Release => {
                    rt.drop_ref(inst);
                    0
                }
                Edge::Ticks(ticks) => rt.add_ticks(inst, ticks, *step),
            };
            if moved == 0 {
                return;
            }
            let up = (moved > 0) != *invert;
            let mut count = moved.unsigned_abs();
            if *max_burst > 0 {
                count = count.min(u32::from(*max_burst));
            }
            let slot = if up { Slot::Up } else { Slot::Down };
            for _ in 0..count {
                fire(rt, def, cx, slot, ctx, false);
            }
        }
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
                State::MultiTap { count, timer, last } if *timer == Some(id) => {
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
        TimerKind::Limiter(_, slot) => on_limiter(rt, def, cx, slot, id),
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
    if !cx.conditions_hold(&def.conditions) {
        let description = match (hold, hold_target) {
            (Some(action), _) => action.describe(&ctx, cx.settings.middle_c_octave),
            (None, Some(target)) => target.hold_label.clone(),
            (None, None) => return,
        };
        cx.report_skip(&rt.info, description, SkipReason::ConditionFailed);
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
    let Some(action) = slot_action(def, &rt.state, slot) else {
        return;
    };
    if !cx.conditions_hold(&def.conditions) {
        let description = action.describe(&ctx, cx.settings.middle_c_octave);
        cx.report_skip(&rt.info, description, SkipReason::ConditionFailed);
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
/// release everything, cancel its timers, forget its instances and pending decisions.
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
        State::MultiTap { count, timer, .. } => {
            cx.timers.cancel_slot(timer);
            *count = 0;
        }
        _ => {}
    }
    for limiter in &mut rt.limiters {
        cx.timers.cancel_slot(&mut limiter.timer);
        limiter.clear();
    }
    rt.instances.clear();
    rt.step_refs.clear();
    rt.limit_reported = None;
    cx.abort_queue(rt.key);
    cx.unhold_keys(rt.key, true);
    cx.release_owner(&rt.info);
    cx.dirty = true;
}

/// A device disconnected: forget its instances and Step references; whatever they were holding
/// is released (without firing release actions); toggles latched from that device unlatch.
pub fn drop_device(rt: &mut MappingRt, def: &CMapping, cx: &mut Cx, device: DeviceId) {
    let before = rt.instances.len();
    rt.instances.retain(|i| i.key.device() != device);
    rt.step_refs.retain(|(k, _)| k.device() != device);
    let removed = rt.instances.len() != before;
    let idle = rt.instances.is_empty();
    match (&def.behavior, &mut rt.state) {
        (CBehavior::Hold { target, .. }, State::Hold { engaged }) if removed && idle && *engaged => {
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
        _ => {}
    }
}
