//! Releasing what this process holds down at the OS when something has gone badly wrong.
//!
//! An injected press outlives the code that made it: on Windows, macOS and X11 a key that was
//! pressed and never released stays down even after the process has died (uinput devices are
//! released by the kernel when they go away). The injectors [`crate::create_injector`] makes
//! record every key and button they press in a table of atomics; [`release_held`] sends a
//! release for each of them straight to the OS, without the injector (whose thread may be the
//! one that panicked, in the middle of a call) and without taking any lock, so it is safe to call
//! from a panic hook. A release for a key that is not down is harmless: a stale entry costs
//! nothing.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use crate::{Injector, Key, MouseButton, Result};

const WORDS: usize = Key::ALL.len().div_ceil(64);

/// Bit `key as usize` is set while `key` may be down.
static KEYS: [AtomicU64; WORDS] = [const { AtomicU64::new(0) }; WORDS];

/// Bit `i` stands for `BUTTONS_ALL[i]`.
static BUTTONS: AtomicU8 = AtomicU8::new(0);

const BUTTONS_ALL: [MouseButton; 5] = [
    MouseButton::Left,
    MouseButton::Right,
    MouseButton::Middle,
    MouseButton::Back,
    MouseButton::Forward,
];

fn record_key(key: Key, down: bool) {
    let index = key as usize;
    let bit = 1u64 << (index % 64);
    if down {
        KEYS[index / 64].fetch_or(bit, Ordering::SeqCst);
    } else {
        KEYS[index / 64].fetch_and(!bit, Ordering::SeqCst);
    }
}

fn record_button(button: MouseButton, down: bool) {
    let Some(index) = BUTTONS_ALL.iter().position(|&b| b == button) else { return };
    let bit = 1u8 << index;
    if down {
        BUTTONS.fetch_or(bit, Ordering::SeqCst);
    } else {
        BUTTONS.fetch_and(!bit, Ordering::SeqCst);
    }
}

/// Sends a release for every key and mouse button an injector pressed and did not release
/// (ordinary keys before modifiers), straight to the OS. Takes no lock and does not allocate.
pub fn release_held() {
    let mut keys = [0u64; WORDS];
    for (word, slot) in keys.iter_mut().zip(KEYS.iter()) {
        *word = slot.swap(0, Ordering::SeqCst);
    }
    let held = |modifier: bool| {
        let keys = &keys;
        (0..WORDS * 64)
            .filter(move |&index| keys[index / 64] & (1 << (index % 64)) != 0)
            .filter_map(|index| Key::ALL.get(index).copied())
            .filter(move |key| key.is_modifier() == modifier)
    };
    let buttons = BUTTONS.swap(0, Ordering::SeqCst);
    let buttons =
        (0..BUTTONS_ALL.len()).filter(move |i| buttons & (1 << i) != 0).map(|i| BUTTONS_ALL[i]);
    crate::backend::emergency_release(held(false).chain(held(true)), buttons);
}

/// Installs a panic hook that calls [`release_held`] before the panic is reported (and before
/// the process aborts, when the panic cannot unwind), then hands over to the previous hook.
///
/// It releases for a panic that can end the process: on the main thread (a command, a window or
/// tray event), in an OS callback, in a library's thread. It leaves alone the panics the process
/// goes on from, where the engine still holds what it holds: those on the threads of
/// midi-2-keyboard itself (named `m2k-...`; the engine releases everything itself when it
/// recovers from one, and the others hold no keys), and those inside a section that catches its
/// own panics, which `caught` reports for the current thread (`m2k_midi::input::catching_panics`:
/// the MIDI input callback). A panic that makes the process abort ([`panic_aborts`]) releases
/// wherever it happens. Installing it twice does nothing.
pub fn install_panic_release(caught: fn() -> bool) {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let ours = std::thread::current().name().is_some_and(|name| name.starts_with("m2k-"));
        if panic_aborts(info) || !(ours || caught()) {
            release_held();
        }
        previous(info);
    }));
}

/// Whether the process aborts after this panic, whatever would have caught it: a panic that
/// reached a function that cannot unwind (an OS callback), or a destructor that panicked while
/// unwinding. Rust reports those with these words, after the hook has seen the panic that led
/// there (checked with Rust 1.99). Should the words change, such a panic counts as any other.
pub fn panic_aborts(info: &std::panic::PanicHookInfo<'_>) -> bool {
    matches!(
        info.payload().downcast_ref::<&str>().copied(),
        Some("panic in a function that cannot unwind" | "panic in a destructor during cleanup")
    )
}

/// An injector whose presses are recorded for [`release_held`].
pub(crate) struct Tracked(pub(crate) Box<dyn Injector>);

impl Injector for Tracked {
    fn name(&self) -> &'static str {
        self.0.name()
    }

    fn supports(&self, key: Key) -> bool {
        self.0.supports(key)
    }

    fn repeats_held_keys(&self) -> bool {
        self.0.repeats_held_keys()
    }

    fn max_text_chunk(&self) -> Option<usize> {
        self.0.max_text_chunk()
    }

    fn key_down(&mut self, key: Key) -> Result<()> {
        // Recorded whatever the outcome: a refused press may still have arrived (macOS posts
        // regardless), and a needless release is harmless.
        record_key(key, true);
        self.0.key_down(key)
    }

    fn key_up(&mut self, key: Key) -> Result<()> {
        let result = self.0.key_up(key);
        // A refused release may have left the key down.
        if result.is_ok() {
            record_key(key, false);
        }
        result
    }

    fn text(&mut self, text: &str) -> Result<()> {
        self.0.text(text)
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()> {
        if down {
            record_button(button, true);
        }
        let result = self.0.mouse_button(button, down);
        if !down && result.is_ok() {
            record_button(button, false);
        }
        result
    }

    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.0.mouse_move_relative(dx, dy)
    }

    fn mouse_move_absolute(&mut self, x: i32, y: i32) -> Result<()> {
        self.0.mouse_move_absolute(x, y)
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()> {
        self.0.scroll(dx, dy)
    }
}
