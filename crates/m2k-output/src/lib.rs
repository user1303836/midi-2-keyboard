//! Key model and OS input injection for midi-2-keyboard.
//!
//! CONTRACT (relied upon by the engine; keep stable):
//!
//! - [`Key`] / [`MouseButton`]: the shared key model (see `key.rs`).
//! - [`Injector`]: the platform abstraction. One implementation per OS backend plus
//!   [`NullInjector`] for dry runs. Implementations are *stateless translators*: they send exactly
//!   the event they are asked to send. Reference counting of held keys, tap timing, auto-repeat,
//!   release-all-on-exit and so on are the engine's job.
//! - [`create_injector`]: builds the best backend for the current platform/session.
//! - [`platform`]: probes the UI needs (permissions, foreground application).

mod emergency;
pub mod key;
mod keymap;
pub mod platform;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

pub use emergency::{install_panic_release, release_held};
pub use key::{Key, MouseButton};

/// Marker carried by every injected event where the platform has room for one (`dwExtraInfo`
/// on Windows, `kCGEventSourceUserData` on macOS). Lets low-level hooks and event taps
/// recognize this application's own events. ASCII `"M2K!"`.
pub const INJECTION_TAG: u32 = 0x4D32_4B21;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    /// The key/button has no representation on this platform/backend.
    #[error("{0} is not supported by the {1} backend")]
    Unsupported(String, &'static str),
    /// The OS refused the injection (missing permission, blocked by security policy, ...).
    #[error("permission denied: {0}")]
    Permission(String),
    /// The backend could not be initialized or lost its connection.
    #[error("output backend unavailable: {0}")]
    Unavailable(String),
    #[error("{0}")]
    Other(String),
    /// The target program runs with higher privileges than midi-2-keyboard (Windows: as
    /// administrator), so the OS keeps our input out of it. The message always contains
    /// [`ELEVATION_REQUIRED`]; [`platform::relaunch_elevated`] is the way out.
    #[error("{0}")]
    Elevated(String),
}

/// Part of every [`OutputError::Elevated`] message, for code that only sees the error text
/// (the engine reports failures as strings).
pub const ELEVATION_REQUIRED: &str = "start MIDI 2 Keyboard as administrator";

pub type Result<T> = std::result::Result<T, OutputError>;

/// How key events are delivered on Windows.
///
/// Every mode presses physical keys: what a key types follows the active keyboard layout, exactly
/// like the real key. In every mode Windows refuses input into windows of programs running as
/// administrator unless midi-2-keyboard runs as administrator too. Brightness, Fast Forward,
/// Rewind, Eject, Power, Wake Up and Num = have no Windows equivalent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum WindowsKeyMode {
    /// Virtual-key code **and** matching scan code in every event. Works for normal applications
    /// and for most games. Numpad digits always type digits, whatever the NumLock state.
    /// Recommended.
    #[default]
    Auto,
    /// Scan codes only (`KEYEVENTF_SCANCODE`): the physical key position, as DirectInput/raw-input
    /// games expect. Layout dependent. Windows then translates numpad keys as navigation keys
    /// (Num 4 acts as Left, Num 7 as Home, even with NumLock on), so use Automatic for numpad
    /// digits. Pause, and keys the active layout does not have, are still sent the Automatic way.
    ScanCode,
    /// Virtual-key codes only. Programs that read scan codes (most games, and apps that tell
    /// left and right modifiers apart that way) may ignore these keys.
    VirtualKey,
}

/// Which injection backend to use on Linux.
///
/// Wayland does not tell applications which window has focus, so per-application profiles and
/// conditions only work in X11 sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum LinuxBackend {
    /// Wayland: uinput when `/dev/uinput` is writable, otherwise X11 XTest through XWayland (then
    /// only X11 applications receive input). X11: XTest, falling back to uinput. Recommended.
    #[default]
    Auto,
    /// A virtual keyboard and mouse (`/dev/uinput`). Works on Wayland and X11 alike and in every
    /// application, but needs write access to `/dev/uinput` (a one-time udev rule; the
    /// permission hint shows the commands). It cannot move the pointer to a screen position,
    /// types text only for ASCII characters assuming a US layout, and long texts must be typed in
    /// pieces.
    Uinput,
    /// The X server's XTest extension. Needs no setup, moves the pointer to screen positions and
    /// types text in any language, but on Wayland it only reaches X11 (XWayland) applications.
    X11,
}

/// Backend selection / tuning. All fields have sensible defaults.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
pub struct InjectorOptions {
    #[serde(default)]
    pub windows_key_mode: WindowsKeyMode,
    #[serde(default)]
    pub linux_backend: LinuxBackend,
}

/// Platform input injection. All methods send immediately and return quickly (no sleeping).
pub trait Injector: Send {
    /// Short backend identifier for diagnostics, e.g. `"windows-sendinput"`, `"macos-cgevent"`,
    /// `"linux-uinput"`, `"linux-x11"`, `"null"`.
    fn name(&self) -> &'static str;

    /// Whether `key` can be sent by this backend.
    fn supports(&self, key: Key) -> bool;

    /// `true` when the OS itself auto-repeats keys that are held down through this backend
    /// (X11, uinput). When `false` (Windows, macOS) the engine emulates keyboard auto-repeat by
    /// calling [`Injector::key_down`] again for the held key.
    fn repeats_held_keys(&self) -> bool {
        false
    }

    /// The most characters one [`Injector::text`] call should carry, when the backend has a
    /// limit (`None`: any length). Longer texts must be split by the caller, with a few
    /// milliseconds between the pieces (one frame, ~16 ms, is always enough). `linux-uinput`
    /// would lose characters otherwise (the kernel queues about 128 events for the compositor,
    /// a character costs 4 to 8); `linux-x11` would mistype them (a keycode it gives a
    /// character must keep it until applications have read the press; the limit follows the
    /// number of free keycodes); `windows-sendinput` keeps each call to about a millisecond
    /// (SendInput blocks the caller per event), so the engine stays responsive. The value may
    /// change while the backend runs: ask before each text.
    fn max_text_chunk(&self) -> Option<usize> {
        None
    }

    /// Presses a key. Calling this for a key that is already down sends another key-down
    /// (an auto-repeat event where the platform distinguishes them).
    fn key_down(&mut self, key: Key) -> Result<()>;

    /// Releases a key.
    fn key_up(&mut self, key: Key) -> Result<()>;

    /// Types Unicode text independent of the keyboard layout where the platform allows it.
    /// `\n` presses Enter and `\t` presses Tab.
    fn text(&mut self, text: &str) -> Result<()>;

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<()>;

    /// Moves the pointer by a relative amount in pixels.
    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<()>;

    /// Moves the pointer to an absolute position in pixels of the virtual desktop.
    fn mouse_move_absolute(&mut self, x: i32, y: i32) -> Result<()>;

    /// Scrolls by wheel notches. Positive `dy` scrolls up (away from the user), positive `dx`
    /// scrolls right.
    fn scroll(&mut self, dx: i32, dy: i32) -> Result<()>;
}

/// An injector that does nothing (dry runs, tests of the pipeline, unsupported platforms).
#[derive(Debug, Default)]
pub struct NullInjector;

impl Injector for NullInjector {
    fn name(&self) -> &'static str {
        "null"
    }
    fn supports(&self, _key: Key) -> bool {
        true
    }
    fn key_down(&mut self, _key: Key) -> Result<()> {
        Ok(())
    }
    fn key_up(&mut self, _key: Key) -> Result<()> {
        Ok(())
    }
    fn text(&mut self, _text: &str) -> Result<()> {
        Ok(())
    }
    fn mouse_button(&mut self, _button: MouseButton, _down: bool) -> Result<()> {
        Ok(())
    }
    fn mouse_move_relative(&mut self, _dx: i32, _dy: i32) -> Result<()> {
        Ok(())
    }
    fn mouse_move_absolute(&mut self, _x: i32, _y: i32) -> Result<()> {
        Ok(())
    }
    fn scroll(&mut self, _dx: i32, _dy: i32) -> Result<()> {
        Ok(())
    }
}

/// Creates the best available injector for this platform and session. What it presses is
/// recorded for [`release_held`] (see [`install_panic_release`]).
///
/// Never panics; when no backend can be started the error says what to do about it.
pub fn create_injector(options: &InjectorOptions) -> Result<Box<dyn Injector>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend::create(options)))
        .unwrap_or_else(|_| {
            Err(OutputError::Unavailable("the output backend crashed while starting".into()))
        })
        .map(|injector| Box::new(emergency::Tracked(injector)) as Box<dyn Injector>)
}

#[cfg(target_os = "linux")]
use linux as backend;
#[cfg(target_os = "macos")]
use macos as backend;
#[cfg(windows)]
use windows as backend;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod backend {
    use super::*;

    pub fn create(_options: &InjectorOptions) -> Result<Box<dyn Injector>> {
        Err(OutputError::Unavailable(
            "keyboard and mouse output is not implemented for this operating system yet".into(),
        ))
    }

    pub fn emergency_release(
        _keys: impl Iterator<Item = Key>,
        _buttons: impl Iterator<Item = MouseButton>,
    ) {
    }
}
