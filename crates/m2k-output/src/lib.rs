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

pub mod key;
mod keymap;
pub mod platform;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

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
}

pub type Result<T> = std::result::Result<T, OutputError>;

/// How key events are delivered on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum WindowsKeyMode {
    /// Virtual-key code **and** matching scan code in every event. Works for normal applications
    /// and for most games. Recommended.
    #[default]
    Auto,
    /// Scan codes only (`KEYEVENTF_SCANCODE`): the physical key position, as DirectInput/raw-input
    /// games expect. Layout dependent.
    ScanCode,
    /// Virtual-key codes only.
    VirtualKey,
}

/// Which injection backend to use on Linux.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum LinuxBackend {
    /// uinput when accessible (works on Wayland and X11), otherwise X11 XTest.
    #[default]
    Auto,
    Uinput,
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

/// Creates the best available injector for this platform and session.
///
/// Never panics; when no backend can be started the error says what to do about it.
pub fn create_injector(options: &InjectorOptions) -> Result<Box<dyn Injector>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| backend::create(options)))
        .unwrap_or_else(|_| {
            Err(OutputError::Unavailable("the output backend crashed while starting".into()))
        })
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
}
