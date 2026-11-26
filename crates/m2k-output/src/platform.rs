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
/// packaged apps.
pub fn foreground_app() -> Option<ForegroundApp> {
    catch_unwind(imp::foreground_app).unwrap_or(None)
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

#[cfg(windows)]
use crate::windows::platform as imp;

#[cfg(not(windows))]
mod imp {
    use super::{ForegroundApp, Permissions};

    pub fn foreground_app() -> Option<ForegroundApp> {
        None
    }

    pub fn permissions() -> Permissions {
        Permissions { can_inject: true, can_detect_foreground: false, hint: String::new() }
    }

    pub fn request_permissions() {}
}
