//! Platform probes used by the engine and the UI.
//!
//! CONTRACT (keep stable): [`foreground_app`] and [`permissions`]. Both must be cheap enough to
//! poll a few times per second and must never panic.

use std::panic::{AssertUnwindSafe, catch_unwind};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// The application that currently has keyboard focus.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
pub struct ForegroundApp {
    /// Executable / process name without directory, e.g. `"Photoshop.exe"`, `"Ableton Live 12"`,
    /// `"firefox"`. Empty when unknown.
    pub process: String,
    /// Title of the focused window. Empty when unknown or unavailable.
    pub title: String,
    /// Platform specific stable identifier when one exists (macOS bundle id, X11 WM_CLASS).
    pub app_id: String,
    /// Process id, 0 when unknown.
    pub pid: u32,
}

/// State of the OS permissions input injection depends on.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
pub struct Permissions {
    /// `true` when injected input will actually reach other applications.
    pub can_inject: bool,
    /// `true` when the foreground application can be detected (per-app profiles/conditions).
    pub can_detect_foreground: bool,
    /// Human readable, actionable explanation when something is missing, otherwise empty.
    /// (macOS: Accessibility permission; Linux: /dev/uinput access, Wayland limitations.)
    pub hint: String,
}

/// Returns the application that currently has keyboard focus, if it can be determined.
///
/// Windows: the process owning the foreground window (for UWP apps hosted by
/// `ApplicationFrameHost.exe`, the app's own process); `app_id` is the AppUserModelID of
/// packaged apps. macOS: `NSWorkspace`'s frontmost application (`app_id` = bundle id), the
/// window title only with the Accessibility permission. Linux X11: the EWMH active window
/// (`app_id` = WM_CLASS class); Wayland: always `None`.
pub fn foreground_app() -> Option<ForegroundApp> {
    catch_unwind(imp::foreground_app).unwrap_or(None)
}

/// Applications that have a visible top-level window, one entry per application (process),
/// sorted by name: the choices for "pick an application" lists. Takes a few milliseconds, so
/// call it on demand rather than in a polling loop; never panics.
///
/// Windows: visible, unowned, non-tool, non-cloaked windows (UWP apps resolved like in
/// [`foreground_app`]). macOS: the regular (Dock) applications, without window titles. Linux
/// X11: the window manager's client list without docks, desktops and windows hidden from the
/// taskbar. Wayland: empty (clients cannot see other windows there).
pub fn running_apps() -> Vec<ForegroundApp> {
    catch_unwind(imp::running_apps).unwrap_or_default()
}

/// Reports whether the permissions needed for input injection are in place.
pub fn permissions() -> Permissions {
    catch_unwind(imp::permissions).unwrap_or_else(|_| Permissions {
        can_inject: false,
        can_detect_foreground: false,
        hint: "The permission check failed unexpectedly.".into(),
    })
}

/// Asks the OS to show its permission prompt / settings pane when one exists (macOS
/// Accessibility). No-op elsewhere.
pub fn request_permissions() {
    let _ = catch_unwind(AssertUnwindSafe(imp::request_permissions));
}

#[cfg(target_os = "linux")]
use crate::linux::platform as imp;
#[cfg(target_os = "macos")]
use crate::macos::platform as imp;
#[cfg(windows)]
use crate::windows::platform as imp;

#[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
mod imp {
    use super::{ForegroundApp, Permissions};

    pub fn foreground_app() -> Option<ForegroundApp> {
        None
    }

    pub fn running_apps() -> Vec<ForegroundApp> {
        Vec::new()
    }

    pub fn permissions() -> Permissions {
        Permissions { can_inject: true, can_detect_foreground: false, hint: String::new() }
    }

    pub fn request_permissions() {}
}
