//! macOS platform probes.
//!
//! The frontmost application comes from `NSWorkspace`, which AppKit keeps current through the
//! main run loop (always running in the app; a bare CLI without a run loop may see a stale
//! value). The focused window's title needs the Accessibility permission and is read with a
//! short messaging timeout so a hung application cannot stall the poller.

use std::collections::HashSet;
use std::ptr::NonNull;

use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSApplicationActivationPolicy, NSWorkspace};
use objc2_application_services::{
    AXError, AXIsProcessTrusted, AXIsProcessTrustedWithOptions, AXUIElement,
    kAXTrustedCheckOptionPrompt,
};
use objc2_core_foundation::{CFBoolean, CFDictionary, CFRetained, CFString, CFType};

use crate::platform::{ForegroundApp, Permissions};

pub(crate) const ACCESSIBILITY_HINT: &str = "midi-2-keyboard needs the Accessibility \
     permission to send keystrokes and mouse input. Open System Settings > Privacy & Security > \
     Accessibility and switch on midi-2-keyboard. If it is already on but input still does not \
     arrive (for example after an update), remove it with the minus button and add it again.";

pub(crate) const SECURE_INPUT_HINT: &str = "macOS is in secure input mode: a password field, \
     or a terminal with Secure Keyboard Entry turned on, has focus. macOS ignores keystrokes from \
     other programs until that field loses focus.";

/// Seconds an Accessibility request to another application may take.
const AX_TIMEOUT: f32 = 0.15;

pub(crate) fn foreground_app() -> Option<ForegroundApp> {
    autoreleasepool(|_| {
        let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
        let pid = app.processIdentifier();
        let process = app.localizedName().map(|name| name.to_string()).unwrap_or_default();
        let app_id = app.bundleIdentifier().map(|id| id.to_string()).unwrap_or_default();
        let title = if pid > 0 && unsafe { AXIsProcessTrusted() } {
            focused_window_title(pid).unwrap_or_default()
        } else {
            String::new()
        };
        let pid = u32::try_from(pid).unwrap_or(0);
        Some(ForegroundApp { process, title, app_id, pid, elevated: false })
    })
}

pub(crate) fn running_apps() -> Vec<ForegroundApp> {
    autoreleasepool(|_| {
        let mut seen = HashSet::new();
        let mut apps = Vec::new();
        for app in NSWorkspace::sharedWorkspace().runningApplications().iter() {
            if app.activationPolicy() != NSApplicationActivationPolicy::Regular {
                continue; // background agents and menu-bar extras
            }
            let process = app.localizedName().map(|name| name.to_string()).unwrap_or_default();
            let app_id = app.bundleIdentifier().map(|id| id.to_string()).unwrap_or_default();
            let key = if app_id.is_empty() { process.to_lowercase() } else { app_id.clone() };
            if process.is_empty() || !seen.insert(key) {
                continue;
            }
            let pid = u32::try_from(app.processIdentifier()).unwrap_or(0);
            apps.push(ForegroundApp {
                process,
                title: String::new(),
                app_id,
                pid,
                elevated: false,
            });
        }
        apps.sort_by_cached_key(|app| app.process.to_lowercase());
        apps
    })
}

pub(crate) fn permissions() -> Permissions {
    let trusted = unsafe { AXIsProcessTrusted() };
    Permissions {
        can_inject: trusted,
        can_detect_foreground: true,
        hint: if trusted { String::new() } else { ACCESSIBILITY_HINT.to_string() },
    }
}

/// Shows the system prompt that leads to the Accessibility settings (only while the
/// permission is missing; macOS ignores repeated requests).
pub(crate) fn request_permissions() {
    // SAFETY: an immutable CFString constant exported by HIServices.
    let key: &CFString = unsafe { kAXTrustedCheckOptionPrompt };
    let options = CFDictionary::<CFString, CFBoolean>::from_slices(&[key], &[CFBoolean::new(true)]);
    // SAFETY: the dictionary maps CFString keys to CFBoolean values, as the API expects.
    unsafe { AXIsProcessTrustedWithOptions(Some(options.as_opaque())) };
}

/// macOS has no privilege separation between applications' input (no UIPI).
pub(crate) fn is_elevated() -> bool {
    false
}

fn focused_window_title(pid: i32) -> Option<String> {
    // SAFETY: AXUIElementCreateApplication accepts any pid.
    let app = unsafe { AXUIElement::new_application(pid) };
    // SAFETY: plain setter on a valid element.
    unsafe { app.set_messaging_timeout(AX_TIMEOUT) };
    let window = copy_attribute(&app, "AXFocusedWindow")?.downcast::<AXUIElement>().ok()?;
    unsafe { window.set_messaging_timeout(AX_TIMEOUT) };
    let title = copy_attribute(&window, "AXTitle")?.downcast::<CFString>().ok()?;
    Some(title.to_string())
}

/// `AXUIElementCopyAttributeValue`, owning the result (Create/Copy rule).
fn copy_attribute(element: &AXUIElement, name: &'static str) -> Option<CFRetained<CFType>> {
    let attribute = CFString::from_static_str(name);
    let mut value: *const CFType = std::ptr::null();
    // SAFETY: `value` is a valid out-pointer for the duration of the call.
    let error = unsafe { element.copy_attribute_value(&attribute, NonNull::from(&mut value)) };
    if error != AXError::Success {
        return None;
    }
    // SAFETY: on success the caller owns one reference to the returned value.
    NonNull::new(value.cast_mut()).map(|value| unsafe { CFRetained::from_raw(value) })
}
