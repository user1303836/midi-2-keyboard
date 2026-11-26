//! Latency measurement: for each MIDI message that produces output right away, the time from
//! `Engine::midi` being called (on the caller's thread) to the return of the first backend call
//! it caused. Samples go into a fixed ring (no allocation once warm); statistics over the last
//! few seconds are published into the status at most once a second while there is traffic.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::events::LatencyStats;

/// Samples kept (the newest ones).
const CAPACITY: usize = 1024;
/// Statistics cover the samples of this long.
const WINDOW: Duration = Duration::from_secs(5);
/// Published statistics change at most this often.
const PUBLISH_INTERVAL: Duration = Duration::from_secs(1);

pub struct LatencyMeter {
    /// (when it was taken, microseconds), oldest first.
    samples: VecDeque<(Instant, u32)>,
    /// Reused for sorting.
    scratch: Vec<u32>,
    /// Samples arrived since the last publication.
    fresh: bool,
    last_publish: Option<Instant>,
    /// The last published value was `Some`.
    published_some: bool,
}

impl Default for LatencyMeter {
    fn default() -> Self {
        LatencyMeter {
            samples: VecDeque::with_capacity(CAPACITY),
            scratch: Vec::with_capacity(CAPACITY),
            fresh: false,
            last_publish: None,
            published_some: false,
        }
    }
}

impl LatencyMeter {
    pub fn record(&mut self, at: Instant, latency: Duration) {
        if self.samples.len() == CAPACITY {
            self.samples.pop_front();
        }
        let micros = u32::try_from(latency.as_micros()).unwrap_or(u32::MAX);
        self.samples.push_back((at, micros));
        self.fresh = true;
    }

    /// What to publish at `now`, if anything is due: `Some(stats)` (stats `None` when the
    /// window is empty).
    pub fn publish(&mut self, now: Instant) -> Option<Option<LatencyStats>> {
        while self
            .samples
            .front()
            .is_some_and(|(at, _)| now.saturating_duration_since(*at) > WINDOW)
        {
            self.samples.pop_front();
        }
        let due = self
            .last_publish
            .is_none_or(|at| now.saturating_duration_since(at) >= PUBLISH_INTERVAL);
        if self.fresh && due {
            self.fresh = false;
            self.last_publish = Some(now);
            let stats = self.stats();
            self.published_some = stats.is_some();
            return Some(stats);
        }
        if self.published_some && self.samples.is_empty() {
            self.published_some = false;
            return Some(None);
        }
        None
    }

    /// When [`LatencyMeter::publish`] will have something new to say without further traffic
    /// (pending fresh samples, or the window running empty). `None` = nothing pending.
    pub fn next_due(&self) -> Option<Instant> {
        if self.fresh {
            return Some(self.last_publish.map_or_else(Instant::now, |at| at + PUBLISH_INTERVAL));
        }
        if self.published_some {
            return self.samples.back().map(|(at, _)| *at + WINDOW);
        }
        None
    }

    fn stats(&mut self) -> Option<LatencyStats> {
        if self.samples.is_empty() {
            return None;
        }
        self.scratch.clear();
        self.scratch.extend(self.samples.iter().map(|(_, micros)| *micros));
        self.scratch.sort_unstable();
        let n = self.scratch.len();
        let p99 = (n * 99).div_ceil(100).clamp(1, n) - 1;
        Some(LatencyStats {
            samples: n as u32,
            median_us: self.scratch[n / 2],
            p99_us: self.scratch[p99],
            max_us: self.scratch[n - 1],
        })
    }
}
