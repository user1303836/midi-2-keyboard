//! The engine's timer queue.
//!
//! Every delay in the engine is an entry here: tap durations, key stagger, macro delays, text
//! typed character by character, repeats, keyboard auto-repeat, rate limits, tap-hold and
//! multi-tap decisions, status coalescing. The engine thread waits on its command channel until
//! the earliest deadline and never sleeps.
//!
//! Entries are ordered by `(deadline, sequence)`: equal deadlines fire in the order they were
//! scheduled, and any entry can be cancelled in `O(log n)`. (A binary heap cannot remove an
//! arbitrary entry; restarting a debounce on every message of a fast knob would leave thousands
//! of dead entries behind it.) Owners keep the [`TimerId`] of what they scheduled and compare it
//! when a timer fires, so a stale entry can never act on state that moved on.

use std::collections::BTreeMap;
use std::time::Instant;

/// Identifies one scheduled entry. Also carries its deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TimerId {
    at: Instant,
    seq: u64,
}

impl TimerId {
    /// The deadline this entry was scheduled for.
    pub fn at(self) -> Instant {
        self.at
    }
}

pub struct Timers<K> {
    queue: BTreeMap<TimerId, K>,
    seq: u64,
}

impl<K> Timers<K> {
    pub fn new() -> Self {
        Timers { queue: BTreeMap::new(), seq: 0 }
    }

    pub fn schedule(&mut self, at: Instant, kind: K) -> TimerId {
        self.seq += 1;
        let id = TimerId { at, seq: self.seq };
        self.queue.insert(id, kind);
        id
    }

    pub fn cancel(&mut self, id: TimerId) {
        self.queue.remove(&id);
    }

    /// Cancels the timer stored in `slot` (if any) and clears the slot.
    pub fn cancel_slot(&mut self, slot: &mut Option<TimerId>) {
        if let Some(id) = slot.take() {
            self.cancel(id);
        }
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        self.queue.first_key_value().map(|(id, _)| id.at)
    }

    /// Removes and returns the earliest entry if it is due at `now`.
    pub fn pop_due(&mut self, now: Instant) -> Option<(TimerId, K)> {
        let (first, _) = self.queue.first_key_value()?;
        if first.at > now {
            return None;
        }
        self.queue.pop_first()
    }

    pub fn clear(&mut self) {
        self.queue.clear();
    }
}
