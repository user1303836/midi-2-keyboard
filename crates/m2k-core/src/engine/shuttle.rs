//! The `Shuttle` behavior: an action repeated for as long as a control is pushed away from where
//! it rests, faster the further away (a pitch wheel that scrolls, a shuttle ring, a joystick axis
//! moving the pointer, a pad that fires faster the harder it is pressed).
//!
//! The arithmetic is the editor's (`shuttleRestZone` / `shuttleAt` / `shuttleRate` in
//! `src/lib/model.ts`, which draw its preview): the travel is the source's range (a note's
//! velocity range, 1..=127 for a chord); the rest position its low end, its middle
//! (`(min + max + 1) / 2`) or its high end; the dead zone `dead_zone` percent of each side's span,
//! rounded up. Beyond it, `d` is how far into the rest of that side's travel the value is
//! (0 < d <= 1), and `rate = 1/slow + (1/fast - 1/slow) * f(d)`, `f(d) = d` or `d * d`.
//!
//! Each matching instance (a note, a channel, a device) repeats on its own. Leaving the dead zone
//! fires at once, but never sooner after the previous firing than the pace of the position allows
//! (an analog control hovering at the edge cannot beat `slow_ms`); a new value re-times the next
//! firing from the last one. A note or a chord is different: each hit is a deliberate, discrete
//! event and fires at once, with no memory of the previous one. Intervals shorter than [`MIN_WAKE`] are not woken for one by one: the
//! engine wakes every few milliseconds and makes up what fell due (at most [`MAX_BATCH`] at
//! once), and a pointer move or a scroll then goes out as one call carrying all of them. An
//! action that queues (key taps, macros, text) never has more than one firing waiting, so letting
//! go stops it at once (a firing that has not started is dropped). Conditions are checked at
//! every firing, quietly. The monitor gets the first firing of a repetition as a row of its own,
//! then at most one row per [`REPORT_EVERY`] per mapping, marked with how many firings it stands
//! for (`Right ×5`), and a last row with the rest when it stops, so the rows add up.

use std::time::{Duration, Instant};

use super::compile::{ActionKind, CAction, CBehavior, CMapping, Prim};
use super::cx::{Ctx, Cx, OwnerInfo, TimerKind};
use super::source::{DeviceId, InstKey};
use super::timers::TimerId;
use crate::events::Outcome;
use crate::model::{ShuttleCurve, ShuttleRest, ValueRange};

/// Shortest time between two wakes of one repetition.
const MIN_WAKE: Duration = Duration::from_millis(4);

/// The most firings one wake makes up for: a system that stalled does not fire a burst.
const MAX_BATCH: u32 = 50;

/// At most one monitor row per mapping per this long while it repeats.
const REPORT_EVERY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Up,
    Down,
}

/// A shuttle's travel: the rest position, the dead zone around it and the pace.
#[derive(Debug, Clone, Copy)]
pub struct Zone {
    min: u16,
    max: u16,
    /// The edges of the dead zone (inclusive): at rest in `quiet_low..=quiet_high`.
    quiet_low: u16,
    quiet_high: u16,
    slow_ms: u32,
    fast_ms: u32,
    curve: ShuttleCurve,
}

impl Zone {
    pub fn new(
        travel: ValueRange,
        rest: ShuttleRest,
        dead_zone: u8,
        slow_ms: u32,
        fast_ms: u32,
        curve: ShuttleCurve,
    ) -> Zone {
        let (min, max) = (travel.min.min(travel.max), travel.max.max(travel.min));
        let rest = match rest {
            ShuttleRest::Low => min,
            ShuttleRest::High => max,
            ShuttleRest::Center => (u32::from(min) + u32::from(max)).div_ceil(2) as u16,
        };
        let dead = |span: u16| ((u32::from(span) * u32::from(dead_zone)).div_ceil(100)) as u16;
        Zone {
            min,
            max,
            quiet_low: rest - dead(rest - min),
            quiet_high: rest + dead(max - rest),
            slow_ms: slow_ms.max(1),
            fast_ms: fast_ms.max(1),
            curve,
        }
    }

    /// Where `value` is: at rest (`None`, also outside the travel), or on a side with the
    /// fraction of that side's travel beyond the dead zone (0 < d <= 1).
    pub fn locate(&self, value: i32) -> Option<(Side, f64)> {
        let (min, max) = (i32::from(self.min), i32::from(self.max));
        let (low, high) = (i32::from(self.quiet_low), i32::from(self.quiet_high));
        if value < min || value > max || (low..=high).contains(&value) {
            return None;
        }
        let (side, beyond, room) = if value > high {
            (Side::Up, value - high, max - high)
        } else {
            (Side::Down, low - value, low - min)
        };
        Some((side, if room > 0 { f64::from(beyond) / f64::from(room) } else { 1.0 }))
    }

    /// The time between firings at `d`.
    pub fn interval(&self, d: f64) -> Duration {
        let (slow, fast) = (1.0 / f64::from(self.slow_ms), 1.0 / f64::from(self.fast_ms));
        let d = d.clamp(0.0, 1.0);
        let f = match self.curve {
            ShuttleCurve::Linear => d,
            ShuttleCurve::Squared => d * d,
        };
        // Firings per millisecond.
        let rate = slow + (fast - slow) * f;
        Duration::from_secs_f64(1.0 / rate / 1000.0)
    }

    fn slowest(&self) -> Duration {
        Duration::from_millis(u64::from(self.slow_ms))
    }
}

/// One instance's repetition.
struct Run {
    key: InstKey,
    /// The side it repeats on; `None` = at rest (kept a while for `last`).
    side: Option<Side>,
    /// The latest message: `{value}` placeholders and the incoming value of "Send MIDI".
    ctx: Ctx,
    interval: Duration,
    /// When it last fired, across rests: hovering at the edge cannot beat the pace.
    last: Option<Instant>,
    timer: Option<TimerId>,
}

/// The runtime state of a `Shuttle` mapping.
#[derive(Default)]
pub struct Shuttle {
    runs: Vec<Run>,
    /// Firings not shown in the monitor yet: how many, of which side, the latest message.
    unreported: u32,
    unreported_side: Option<Side>,
    unreported_ctx: Ctx,
    /// When the last monitor row was made.
    reported: Option<Instant>,
    /// A repetition started: its first firing is reported at once.
    fresh: bool,
    /// A firing of this side is queued behind the one playing: it counts once it starts (and
    /// never, when stopping drops it).
    waiting: Option<Side>,
}

impl Shuttle {
    /// Repeating somewhere (the mapping shows as on).
    pub fn engaged(&self) -> bool {
        self.runs.iter().any(|run| run.side.is_some())
    }

    /// A new position of instance `key` (a press, a move). `press`: a new press of the instance
    /// (for a note or a chord: a hit, which starts afresh).
    pub fn moved(
        &mut self,
        def: &CMapping,
        info: &OwnerInfo,
        cx: &mut Cx,
        key: InstKey,
        ctx: Ctx,
        press: bool,
    ) {
        let CBehavior::Shuttle { up, down, zone, hits } = &def.behavior else {
            return;
        };
        let now = cx.now;
        // At rest, outside the travel, or on a side with nothing to do: no repetition.
        let spot = zone.locate(ctx.value).filter(|(side, _)| match side {
            Side::Up => up.is_some(),
            Side::Down => down.is_some(),
        });
        let position = self.runs.iter().position(|run| run.key == key);
        let Some((side, d)) = spot else {
            if let Some(i) = position {
                self.stop(i, def, info, cx);
            }
            return;
        };
        let i = match position {
            Some(i) => i,
            None => {
                self.prune(zone, now);
                let run =
                    Run { key, side: None, ctx, interval: Duration::ZERO, last: None, timer: None };
                self.runs.push(run);
                self.runs.len() - 1
            }
        };
        let run = &mut self.runs[i];
        if press && *hits {
            // A hit fires at once, whatever fired before.
            run.last = None;
        }
        // Leaving the dead zone, or crossing to the other side.
        let starting = run.side != Some(side);
        if starting {
            run.side = Some(side);
            self.fresh = true;
            cx.dirty = true;
        }
        let run = &mut self.runs[i];
        cx.timers.cancel_slot(&mut run.timer);
        // A steady run finer than the wake period still owes what fell due at its pace since its
        // last wake (a fast pointer loses nothing to the messages of a moving wheel). Nothing is
        // ever made up for time that passed at a slower pace.
        if !starting {
            let owed = owed(run.interval, run.last, cx.actual_now());
            if owed > 0 {
                // With the message they fell due under.
                self.fire(i, def, info, cx, owed, true);
            }
        }
        let run = &mut self.runs[i];
        cx.timers.cancel_slot(&mut run.timer);
        run.ctx = ctx;
        run.interval = zone.interval(d);
        // At once (one firing, the pace counts from it), or re-timed from the last firing at the
        // pace of the new value: never sooner.
        let due = run.last.map_or(now, |last| (last + run.interval).max(now));
        if due <= now {
            self.fire(i, def, info, cx, 1, false);
        } else {
            run.timer = Some(cx.timers.schedule(due, TimerKind::Shuttle(info.owner, key)));
        }
    }

    /// Instance `key` went inactive (note off, the value left the range).
    pub fn released(&mut self, def: &CMapping, info: &OwnerInfo, cx: &mut Cx, key: InstKey) {
        if let Some(i) = self.runs.iter().position(|run| run.key == key) {
            self.stop(i, def, info, cx);
            // A note's or a chord's next hit starts afresh: nothing to remember.
            if matches!(def.behavior, CBehavior::Shuttle { hits: true, .. }) {
                self.runs.remove(i);
            }
        }
    }

    /// The timer of instance `key` is due: fire what fell due since the last firing.
    pub fn on_timer(
        &mut self,
        def: &CMapping,
        info: &OwnerInfo,
        cx: &mut Cx,
        key: InstKey,
        id: TimerId,
        live: bool,
    ) {
        let Some(i) = self.runs.iter().position(|run| run.key == key && run.timer == Some(id))
        else {
            return;
        };
        self.runs[i].timer = None;
        if !live || self.runs[i].side.is_none() {
            return;
        }
        let run = &self.runs[i];
        let due = owed(run.interval, run.last, cx.actual_now()).max(1);
        self.fire(i, def, info, cx, due, true);
    }

    /// Stops every repetition (pause, focus suspension, learning, release all): it does not
    /// resume by itself, the next message from the control starts it again.
    pub fn stop_all(&mut self, def: &CMapping, info: &OwnerInfo, cx: &mut Cx) {
        for i in 0..self.runs.len() {
            self.stop(i, def, info, cx);
        }
    }

    /// The device is gone: its repetitions stop and are forgotten.
    pub fn drop_device(&mut self, def: &CMapping, info: &OwnerInfo, cx: &mut Cx, device: DeviceId) {
        for i in 0..self.runs.len() {
            if self.runs[i].key.device() == device {
                self.stop(i, def, info, cx);
            }
        }
        self.runs.retain(|run| run.key.device() != device);
    }

    /// Forgets everything (deactivation).
    pub fn clear(&mut self, def: &CMapping, info: &OwnerInfo, cx: &mut Cx) {
        self.stop_all(def, info, cx);
        *self = Shuttle::default();
    }

    fn stop(&mut self, i: usize, def: &CMapping, info: &OwnerInfo, cx: &mut Cx) {
        let run = &mut self.runs[i];
        cx.timers.cancel_slot(&mut run.timer);
        if run.side.take().is_none() {
            return;
        }
        cx.dirty = true;
        if !self.engaged() {
            if let Some(side) = self.waiting.take() {
                if cx.exec.has_pending(info.owner) {
                    // Nothing is left to play out: the firing that has not started is dropped.
                    cx.exec.drop_pending(info.owner, |_| true);
                } else {
                    self.add(def, info, cx, side, 1);
                }
            }
            // The firings since the last row get theirs.
            self.flush(def, info, cx);
        }
    }

    /// Counts `n` firings of `side` for the monitor (a row is about one side).
    fn add(&mut self, def: &CMapping, info: &OwnerInfo, cx: &mut Cx, side: Side, n: u32) {
        if self.unreported_side.is_some_and(|other| other != side) {
            self.flush(def, info, cx);
        }
        self.unreported = self.unreported.saturating_add(n);
        self.unreported_side = Some(side);
    }

    /// Reports the firings not shown yet as one row (`Right ×7`).
    fn flush(&mut self, def: &CMapping, info: &OwnerInfo, cx: &mut Cx) {
        let count = std::mem::take(&mut self.unreported);
        let Some(side) = self.unreported_side.take() else {
            return;
        };
        let CBehavior::Shuttle { up, down, .. } = &def.behavior else {
            return;
        };
        let action = match side {
            Side::Up => up.as_ref(),
            Side::Down => down.as_ref(),
        };
        let (Some(action), true) = (action, count > 0 && cx.monitor) else {
            return;
        };
        let mut description = cx.describe(action, &self.unreported_ctx);
        if count > 1 {
            description.push_str(&format!(" \u{00D7}{count}"));
        }
        cx.report(info, description, Outcome::Sent);
        self.reported = Some(cx.actual_now());
    }

    /// Repetitions at rest whose last firing can no longer hold anything back.
    fn prune(&mut self, zone: &Zone, now: Instant) {
        let slowest = zone.slowest();
        self.runs.retain(|run| {
            run.side.is_some()
                || run.last.is_some_and(|last| now.saturating_duration_since(last) < slowest)
        });
    }

    /// Fires run `i`: `count` firings fell due (made up for in one go). `catch_up`: from its
    /// timer (the cadence goes on from the last firing) rather than at once.
    fn fire(
        &mut self,
        i: usize,
        def: &CMapping,
        info: &OwnerInfo,
        cx: &mut Cx,
        count: u32,
        catch_up: bool,
    ) {
        let CBehavior::Shuttle { up, down, .. } = &def.behavior else {
            return;
        };
        let real = cx.actual_now();
        let run = &mut self.runs[i];
        let Some(side) = run.side else {
            return;
        };
        let Some(action) = (match side {
            Side::Up => up.as_ref(),
            Side::Down => down.as_ref(),
        }) else {
            return;
        };
        // From its timer the cadence goes on exactly: a batch's remainder carries over, an
        // on-time firing does not drift by the wake-up delay. At once, re-timed, after a stall,
        // or late by more than an interval: from now.
        let last = match run.last {
            Some(last) if catch_up && run.interval < MIN_WAKE && count < MAX_BATCH => {
                last + run.interval * count
            }
            Some(last)
                if catch_up
                    && real.saturating_duration_since(last + run.interval) < run.interval =>
            {
                last + run.interval
            }
            _ => real,
        };
        run.last = Some(last);
        // Faster paces are batched: wake every few milliseconds, not for every firing.
        let next = last + run.interval.max(MIN_WAKE);
        run.timer = Some(cx.timers.schedule(next, TimerKind::Shuttle(info.owner, run.key)));
        let ctx = run.ctx;

        // A firing that was waiting to start has started meanwhile: it counts now.
        if let Some(waited) = self.waiting
            && !cx.exec.has_pending(info.owner)
        {
            self.waiting = None;
            self.add(def, info, cx, waited, 1);
        }
        if cx.failed_condition(&def.conditions).is_some() {
            return;
        }
        let Some((done, waiting)) = perform(cx, info, action, ctx, count) else {
            return;
        };
        if waiting {
            self.waiting = Some(side);
        }
        self.unreported_ctx = ctx;
        self.add(def, info, cx, side, done);
        let due = self.fresh
            || self.reported.is_none_or(|at| real.saturating_duration_since(at) >= REPORT_EVERY);
        if due {
            self.fresh = false;
            self.flush(def, info, cx);
        }
    }
}

/// Firings owed at `interval` since `last`: only a pace finer than the wake period is made up
/// (in batches, at most [`MAX_BATCH`]); a slower one fires once, when it is due.
fn owed(interval: Duration, last: Option<Instant>, now: Instant) -> u32 {
    match last {
        Some(last) if interval < MIN_WAKE => {
            let elapsed = now.saturating_duration_since(last).as_nanos();
            (elapsed / interval.as_nanos().max(1)).min(u128::from(MAX_BATCH)) as u32
        }
        _ => 0,
    }
}

/// Pointer moves and scrolls only: a batch of them is one call carrying all of it.
fn batchable(action: &CAction) -> Option<&[Prim]> {
    match &action.kind {
        ActionKind::Output(prims)
            if !prims.is_empty()
                && prims.iter().all(|p| matches!(p, Prim::Move { .. } | Prim::Scroll { .. })) =>
        {
            Some(prims)
        }
        _ => None,
    }
}

/// Performs `count` firings of `action` (quietly: the shuttle makes its own monitor rows).
/// Returns how many firings went out now and whether one is waiting to start behind the action
/// that is playing; `None` when nothing went out.
fn perform(
    cx: &mut Cx,
    info: &OwnerInfo,
    action: &CAction,
    ctx: Ctx,
    count: u32,
) -> Option<(u32, bool)> {
    if cx.gate.check(action.class).is_some() {
        return None;
    }
    if let Some(prims) = batchable(action) {
        let n = i32::try_from(count).unwrap_or(i32::MAX);
        for prim in prims {
            let result = match *prim {
                Prim::Move { x, y, absolute: false } => {
                    cx.out.move_pointer(x.saturating_mul(n), y.saturating_mul(n), false)
                }
                Prim::Move { x, y, absolute: true } => cx.out.move_pointer(x, y, true),
                Prim::Scroll { dx, dy } => {
                    cx.out.scroll(dx.saturating_mul(n), dy.saturating_mul(n))
                }
                _ => Ok(()),
            };
            if let Err(error) = result {
                cx.report_failure(info, action.label().to_string(), error);
            }
        }
        return Some((count, false));
    }
    // An action that queues never has more than one firing waiting.
    if action.queues() && cx.exec.has_pending(info.owner) {
        return None;
    }
    let monitor = std::mem::replace(&mut cx.monitor, false);
    let done = cx.execute(info, action, ctx, true);
    cx.monitor = monitor;
    if !done {
        return None;
    }
    let waiting = action.queues() && cx.exec.has_pending(info.owner);
    Some(if waiting { (0, true) } else { (1, false) })
}
