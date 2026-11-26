//! Rate limiting ([`RateLimit`]): one limiter per (mapping, slot).
//!
//! - Throttle: fire immediately, then swallow for `interval_ms`. With `trailing`, if anything was
//!   swallowed, fire once more when the interval ends, with the latest trigger's data; that
//!   firing starts a new interval.
//! - Debounce: fire once, with the latest trigger's data, after `interval_ms` without triggers.
//!
//! The limiter keeps at most one timer outstanding: when a debounce is pushed back, the existing
//! timer fires early, notices the later deadline and is rescheduled once.

use std::time::{Duration, Instant};

use super::cx::Ctx;
use super::timers::TimerId;
use crate::model::{RateLimit, RateLimitMode};

/// What can be rate limited within one mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Press,
    Release,
    Up,
    Down,
    Change,
    Cycle,
}

pub const SLOTS: usize = 6;

impl Slot {
    pub fn index(self) -> usize {
        match self {
            Slot::Press => 0,
            Slot::Release => 1,
            Slot::Up => 2,
            Slot::Down => 3,
            Slot::Change => 4,
            Slot::Cycle => 5,
        }
    }
}

pub enum Admission {
    /// Fire now.
    Fire,
    /// Swallowed; a deferred firing is due at this time (make sure a timer is set).
    Defer(Instant),
    /// Swallowed for good.
    Drop,
}

#[derive(Debug, Default)]
pub struct Limiter {
    /// Throttle: triggers are swallowed until this time.
    window_end: Option<Instant>,
    /// Trailing throttle / debounce: the latest swallowed trigger.
    pending: Option<Ctx>,
    /// Debounce: when the pending trigger fires.
    due: Option<Instant>,
    pub timer: Option<TimerId>,
}

fn interval(limit: &RateLimit) -> Duration {
    Duration::from_millis(u64::from(limit.interval_ms.max(1)))
}

impl Limiter {
    pub fn admit(&mut self, limit: &RateLimit, now: Instant, ctx: Ctx) -> Admission {
        match limit.mode {
            RateLimitMode::Throttle => match self.window_end {
                Some(end) if now < end => {
                    if limit.trailing {
                        self.pending = Some(ctx);
                        Admission::Defer(end)
                    } else {
                        Admission::Drop
                    }
                }
                _ => {
                    self.window_end = Some(now + interval(limit));
                    Admission::Fire
                }
            },
            RateLimitMode::Debounce => {
                let due = now + interval(limit);
                self.pending = Some(ctx);
                self.due = Some(due);
                Admission::Defer(due)
            }
        }
    }

    /// The limiter's timer fired at `now`. Returns the trigger to fire now (if any) and when
    /// the timer must fire again (if ever).
    pub fn expire(&mut self, limit: &RateLimit, now: Instant) -> (Option<Ctx>, Option<Instant>) {
        match limit.mode {
            RateLimitMode::Throttle => {
                let ctx = self.pending.take();
                if ctx.is_some() {
                    self.window_end = Some(now + interval(limit));
                }
                (ctx, None)
            }
            RateLimitMode::Debounce => match self.due {
                Some(due) if due > now => (None, Some(due)),
                _ => {
                    self.due = None;
                    (self.pending.take(), None)
                }
            },
        }
    }

    /// Forgets everything (the caller cancels `timer`).
    pub fn clear(&mut self) {
        self.window_end = None;
        self.pending = None;
        self.due = None;
    }
}
