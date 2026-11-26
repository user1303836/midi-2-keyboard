//! Platform probes used by the engine and the UI.
//!
//! CONTRACT (keep stable): [`foreground_app`] and [`permissions`]. Both must be cheap enough to
//! poll a few times per second and must never panic.

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
pub fn foreground_app() -> Option<ForegroundApp> {
    None
}

/// Reports whether the permissions needed for input injection are in place.
pub fn permissions() -> Permissions {
    Permissions { can_inject: true, can_detect_foreground: false, hint: String::new() }
}

/// Asks the OS to show its permission prompt / settings pane when one exists (macOS
/// Accessibility). No-op elsewhere.
pub fn request_permissions() {}
