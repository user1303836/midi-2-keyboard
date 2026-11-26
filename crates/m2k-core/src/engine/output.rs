//! Reference-counted output on top of the [`Injector`]: the engine's single record of which keys
//! and mouse buttons it holds down, and on whose behalf.
//!
//! Every holder (a hold engagement, a toggle latch, a running action) is identified by an owner
//! key. A press always reaches the OS (a tap of a key that something else holds must still
//! register as a key down); a release only reaches the OS when the last holder lets go.
//!
//! A release the backend refuses (Windows UIPI while an elevated window has focus, a revoked
//! macOS Accessibility permission, ...) leaves the key down at the OS. Such keys and buttons are
//! kept as *stuck*: they are still reported as held, every [`Output::release_everything`]
//! retries them, and the engine retries them on a timer until the OS accepts the release.
//! Whatever is still held or stuck when the output is dropped is released, so the engine thread
//! cannot exit (or swap backends) with a key down.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use m2k_output::{Injector, Key, MouseButton, OutputError};

use super::latency::LatencyMeter;

#[derive(Debug, Default)]
struct Holders {
    total: u32,
    by: Vec<(u64, u32)>,
}

impl Holders {
    fn add(&mut self, owner: u64) {
        self.total += 1;
        match self.by.iter_mut().find(|(o, _)| *o == owner) {
            Some((_, n)) => *n += 1,
            None => self.by.push((owner, 1)),
        }
    }

    /// Drops one reference of `owner`. `false` if it held none.
    fn remove(&mut self, owner: u64) -> bool {
        let Some(pos) = self.by.iter().position(|(o, _)| *o == owner) else {
            return false;
        };
        self.by[pos].1 -= 1;
        if self.by[pos].1 == 0 {
            self.by.swap_remove(pos);
        }
        self.total -= 1;
        true
    }

    /// Drops every reference of `owner`; returns how many there were.
    fn remove_all(&mut self, owner: u64) -> u32 {
        let Some(pos) = self.by.iter().position(|(o, _)| *o == owner) else {
            return 0;
        };
        let (_, n) = self.by.swap_remove(pos);
        self.total -= n;
        n
    }
}

pub struct Output {
    injector: Box<dyn Injector>,
    keys: BTreeMap<Key, Holders>,
    buttons: BTreeMap<MouseButton, Holders>,
    /// Nobody holds these any more, but the backend refused their release: still down at the
    /// OS until a retry succeeds.
    stuck_keys: BTreeSet<Key>,
    stuck_buttons: BTreeSet<MouseButton>,
    /// Keys / buttons whose press reached the OS since their last successful release. Only
    /// these can be stuck: when the press never got through either (macOS without the
    /// Accessibility permission), a refused release leaves nothing down.
    down_keys: BTreeSet<Key>,
    down_buttons: BTreeSet<MouseButton>,
    /// Set whenever the set of held keys changes (for the status snapshot).
    pub changed: bool,
    /// While a MIDI message is being handled: when it was handed to the engine. The first
    /// backend call that returns takes it and records the latency.
    pub latency_origin: Option<Instant>,
    pub meter: LatencyMeter,
}

type Result = std::result::Result<(), OutputError>;

impl Output {
    pub fn new(injector: Box<dyn Injector>) -> Self {
        Output {
            injector,
            keys: BTreeMap::new(),
            buttons: BTreeMap::new(),
            stuck_keys: BTreeSet::new(),
            stuck_buttons: BTreeSet::new(),
            down_keys: BTreeSet::new(),
            down_buttons: BTreeSet::new(),
            changed: true,
            latency_origin: None,
            meter: LatencyMeter::default(),
        }
    }

    /// Every injection goes through here (latency measurement).
    fn call<T>(&mut self, inject: impl FnOnce(&mut dyn Injector) -> T) -> T {
        let result = inject(self.injector.as_mut());
        if let Some(origin) = self.latency_origin.take() {
            let now = Instant::now();
            self.meter.record(now, now.saturating_duration_since(origin));
        }
        result
    }

    pub fn name(&self) -> &'static str {
        self.injector.name()
    }

    pub fn repeats_held_keys(&self) -> bool {
        self.injector.repeats_held_keys()
    }

    /// Long text has to be sent in small pieces (the uinput backend drops events otherwise).
    pub fn paces_text(&self) -> bool {
        self.injector.name() == "linux-uinput"
    }

    pub fn key_count(&self, key: Key) -> u32 {
        self.keys.get(&key).map_or(0, |h| h.total)
    }

    pub fn button_count(&self, button: MouseButton) -> u32 {
        self.buttons.get(&button).map_or(0, |h| h.total)
    }

    /// Presses `key` for `owner`. Always sends a key down. The reference is counted even when
    /// the backend fails, so the matching release stays balanced.
    pub fn press_key(&mut self, key: Key, owner: u64) -> Result {
        // A stuck key that is pressed again is simply held again; its next release retries.
        self.stuck_keys.remove(&key);
        let holders = self.keys.entry(key).or_default();
        if holders.total == 0 {
            self.changed = true;
        }
        holders.add(owner);
        let result = self.call(|i| i.key_down(key));
        if result.is_ok() {
            self.down_keys.insert(key);
        }
        result
    }

    /// Releases one reference of `owner`; sends the key up when nobody holds the key any more.
    pub fn release_key(&mut self, key: Key, owner: u64) -> Result {
        let Some(holders) = self.keys.get_mut(&key) else {
            return Ok(());
        };
        if !holders.remove(owner) {
            log::debug!("release of {key:?} by an owner that does not hold it");
            return Ok(());
        }
        if holders.total == 0 {
            self.keys.remove(&key);
            return self.key_up(key);
        }
        Ok(())
    }

    /// Sends a key up for a key nobody holds; a refused one becomes stuck if its press had
    /// reached the OS.
    fn key_up(&mut self, key: Key) -> Result {
        let result = self.call(|i| i.key_up(key));
        if result.is_ok() || !self.down_keys.contains(&key) {
            self.stuck_keys.remove(&key);
            self.down_keys.remove(&key);
        } else {
            self.stuck_keys.insert(key);
        }
        self.changed = true;
        result
    }

    /// Keyboard auto-repeat emulation: another key down for a key that is held.
    pub fn repeat_key(&mut self, key: Key) -> Result {
        let result = self.call(|i| i.key_down(key));
        if result.is_ok() {
            self.down_keys.insert(key);
        }
        result
    }

    /// A key up for a key nobody holds (a macro's `KeyUp` of a key it never pressed).
    pub fn key_up_unheld(&mut self, key: Key) -> Result {
        if self.key_count(key) > 0 {
            return Ok(());
        }
        self.key_up(key)
    }

    pub fn press_button(&mut self, button: MouseButton, owner: u64) -> Result {
        self.stuck_buttons.remove(&button);
        self.buttons.entry(button).or_default().add(owner);
        let result = self.call(|i| i.mouse_button(button, true));
        if result.is_ok() {
            self.down_buttons.insert(button);
        }
        result
    }

    pub fn release_button(&mut self, button: MouseButton, owner: u64) -> Result {
        let Some(holders) = self.buttons.get_mut(&button) else {
            return Ok(());
        };
        if !holders.remove(owner) {
            return Ok(());
        }
        if holders.total == 0 {
            self.buttons.remove(&button);
            return self.button_up(button);
        }
        Ok(())
    }

    fn button_up(&mut self, button: MouseButton) -> Result {
        let result = self.call(|i| i.mouse_button(button, false));
        if result.is_ok() || !self.down_buttons.contains(&button) {
            self.stuck_buttons.remove(&button);
            self.down_buttons.remove(&button);
        } else {
            self.stuck_buttons.insert(button);
        }
        result
    }

    pub fn button_up_unheld(&mut self, button: MouseButton) -> Result {
        if self.button_count(button) > 0 {
            return Ok(());
        }
        self.button_up(button)
    }

    pub fn text(&mut self, text: &str) -> Result {
        if text.is_empty() {
            return Ok(());
        }
        self.call(|i| i.text(text))
    }

    pub fn scroll(&mut self, dx: i32, dy: i32) -> Result {
        self.call(|i| i.scroll(dx, dy))
    }

    pub fn move_pointer(&mut self, x: i32, y: i32, absolute: bool) -> Result {
        if absolute {
            self.call(|i| i.mouse_move_absolute(x, y))
        } else {
            self.call(|i| i.mouse_move_relative(x, y))
        }
    }

    /// Safety net: releases every reference `owner` still has (its own bookkeeping should
    /// already have done so).
    pub fn release_owner(&mut self, owner: u64) -> Vec<OutputError> {
        let mut errors = Vec::new();
        let keys: Vec<Key> = self
            .keys
            .iter()
            .filter(|(_, h)| h.by.iter().any(|(o, _)| *o == owner))
            .map(|(k, _)| *k)
            .collect();
        for key in release_order(keys) {
            if let Some(holders) = self.keys.get_mut(&key) {
                holders.remove_all(owner);
                if holders.total == 0 {
                    self.keys.remove(&key);
                    if let Err(error) = self.key_up(key) {
                        errors.push(error);
                    }
                }
            }
        }
        let buttons: Vec<MouseButton> = self
            .buttons
            .iter()
            .filter(|(_, h)| h.by.iter().any(|(o, _)| *o == owner))
            .map(|(b, _)| *b)
            .collect();
        for button in buttons {
            if let Some(holders) = self.buttons.get_mut(&button) {
                holders.remove_all(owner);
                if holders.total == 0 {
                    self.buttons.remove(&button);
                    if let Err(error) = self.button_up(button) {
                        errors.push(error);
                    }
                }
            }
        }
        errors
    }

    /// Releases everything that is held, whoever holds it (non-modifier keys first, then
    /// modifiers, then mouse buttons), and retries what is stuck. Returns the errors of
    /// releases that failed for the first time (repeated failures of stuck ones stay quiet).
    pub fn release_everything(&mut self) -> Vec<OutputError> {
        let mut errors = Vec::new();
        let keys: Vec<Key> = std::mem::take(&mut self.keys).into_keys().collect();
        for key in release_order(keys) {
            if let Err(error) = self.key_up(key) {
                errors.push(error);
            }
        }
        for button in std::mem::take(&mut self.buttons).into_keys() {
            if let Err(error) = self.button_up(button) {
                errors.push(error);
            }
        }
        self.retry_stuck();
        errors
    }

    /// Tries again to release what the backend refused earlier. Returns whether something is
    /// still stuck.
    pub fn retry_stuck(&mut self) -> bool {
        let keys: Vec<Key> = self.stuck_keys.iter().copied().collect();
        for key in release_order(keys) {
            let _ = self.key_up(key);
        }
        let buttons: Vec<MouseButton> = self.stuck_buttons.iter().copied().collect();
        for button in buttons {
            let _ = self.button_up(button);
        }
        self.has_stuck()
    }

    pub fn has_stuck(&self) -> bool {
        !self.stuck_keys.is_empty() || !self.stuck_buttons.is_empty()
    }

    /// Keys down at the OS because of the engine: held ones and stuck ones.
    pub fn held_keys(&self) -> Vec<Key> {
        let mut keys: Vec<Key> = self.keys.keys().copied().collect();
        keys.extend(self.stuck_keys.iter().copied());
        keys.sort();
        keys.dedup();
        keys
    }

    /// Swaps the backend. Call [`Output::release_everything`] first; keys the old backend could
    /// not release are retried through the new one (both inject into the same OS).
    pub fn replace_injector(&mut self, injector: Box<dyn Injector>) -> Box<dyn Injector> {
        self.changed = true;
        std::mem::replace(&mut self.injector, injector)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        let _ = self.release_everything();
    }
}

/// Non-modifier keys first (in reverse order), modifiers last, as when releasing a combination.
fn release_order(mut keys: Vec<Key>) -> Vec<Key> {
    keys.reverse();
    keys.sort_by_key(|k| k.is_modifier());
    keys
}
