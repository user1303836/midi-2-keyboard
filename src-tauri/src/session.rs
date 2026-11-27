//! Sleep, lock and user switching.
//!
//! Windows: when the machine is about to sleep, the session locks, or it is switched away from
//! (fast user switching, a remote session disconnecting), everything held is released. Input
//! sent while locked or away is refused (and its releases retried until it is back), and a key
//! held across a sleep could stay down for good: a USB controller that comes back under the same
//! name is not seen to have gone, so the release of its pad never arrives. Back from sleep,
//! unlocked or reconnected, the MIDI ports in use are closed and opened again
//! (`Runtime::reopen_midi`): the handle of such a controller is dead, and a scan alone sees
//! nothing to do.
//!
//! Linux: the lock screen receives XTest and uinput input like any window, so a key left down or
//! a shuttle left repeating would type into the password field. Before a sleep and when the
//! session locks (logind, see `imp`), everything held is released and shuttles stop (they start
//! again only when their control moves); after waking and on unlock, the ports are reopened.
//! Nothing is held back while locked: there is no gate.
//!
//! macOS: not watched. Input posted from a session in the background (fast user switching) is
//! dropped without an error. Linux: uinput reaches whichever session is in front, so a held key
//! can reach another user's session after a switch.

#[cfg(any(windows, target_os = "linux"))]
pub use shared::watch;

#[cfg(not(any(windows, target_os = "linux")))]
pub fn watch(_app: tauri::AppHandle) {}

#[cfg(any(windows, target_os = "linux"))]
mod shared {
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::OnceLock;

    use tauri::{AppHandle, Manager};

    use crate::AppState;

    static APP: OnceLock<AppHandle> = OnceLock::new();

    #[derive(Clone, Copy)]
    pub(super) enum Change {
        /// Sleep, lock, switched away.
        Away,
        /// Resume, unlock, switched back.
        Back,
    }

    /// Starts watching (once), on a thread of its own.
    pub fn watch(app: AppHandle) {
        if APP.set(app).is_err() {
            return;
        }
        let spawned =
            std::thread::Builder::new().name("m2k-session-watch".into()).spawn(super::imp::run);
        if let Err(error) = spawned {
            log::warn!("could not watch for sleep and lock: {error}");
        }
    }

    /// Acts on a change. Called from OS callbacks, so it never unwinds.
    pub(super) fn apply(change: Change) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let Some(state) = APP.get().and_then(|app| app.try_state::<AppState>()) else {
                return;
            };
            match change {
                Change::Away => state.runtime.release_all(),
                Change::Back => state.runtime.reopen_midi(),
            }
        }));
    }
}

#[cfg(windows)]
mod imp {
    use std::mem::zeroed;
    use std::ptr::{null, null_mut};

    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::RemoteDesktop::{
        NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, MSG,
        PBT_APMRESUMEAUTOMATIC, PBT_APMSUSPEND, RegisterClassW, TranslateMessage,
        WM_POWERBROADCAST, WM_WTSSESSION_CHANGE, WNDCLASSW, WS_OVERLAPPED, WTS_CONSOLE_CONNECT,
        WTS_CONSOLE_DISCONNECT, WTS_REMOTE_CONNECT, WTS_REMOTE_DISCONNECT, WTS_SESSION_LOCK,
        WTS_SESSION_UNLOCK,
    };

    use super::shared::{Change, apply};

    /// The watcher thread: a window of its own and its message loop.
    pub(super) fn run() {
        // SAFETY: a window class and a window owned by this thread, and its message loop.
        unsafe { watch_messages() }
    }

    unsafe fn watch_messages() {
        let instance = unsafe { GetModuleHandleW(null()) };
        let class: Vec<u16> = "m2k-session-watch\0".encode_utf16().collect();
        let mut window_class: WNDCLASSW = unsafe { zeroed() };
        window_class.lpfnWndProc = Some(window_proc);
        window_class.hInstance = instance;
        window_class.lpszClassName = class.as_ptr();
        if unsafe { RegisterClassW(&window_class) } == 0 {
            return;
        }
        // A top-level window that is never shown: power broadcasts do not reach message-only
        // windows.
        let window = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                class.as_ptr(),
                WS_OVERLAPPED,
                0,
                0,
                0,
                0,
                null_mut(),
                null_mut(),
                instance,
                null(),
            )
        };
        if window.is_null() {
            return;
        }
        unsafe { WTSRegisterSessionNotification(window, NOTIFY_FOR_THIS_SESSION) };
        let mut message: MSG = unsafe { zeroed() };
        while unsafe { GetMessageW(&mut message, null_mut(), 0, 0) } > 0 {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    unsafe extern "system" fn window_proc(
        window: HWND,
        message: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let change = match (message, wparam as u32) {
            (WM_POWERBROADCAST, PBT_APMSUSPEND) => Some(Change::Away),
            (WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC) => Some(Change::Back),
            (
                WM_WTSSESSION_CHANGE,
                WTS_SESSION_LOCK | WTS_CONSOLE_DISCONNECT | WTS_REMOTE_DISCONNECT,
            ) => Some(Change::Away),
            (
                WM_WTSSESSION_CHANGE,
                WTS_SESSION_UNLOCK | WTS_CONSOLE_CONNECT | WTS_REMOTE_CONNECT,
            ) => Some(Change::Back),
            _ => None,
        };
        if let Some(change) = change {
            apply(change);
            if message == WM_POWERBROADCAST {
                return 1;
            }
        }
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }
}

/// logind (systemd-logind or elogind) on the system bus, through gio, which GTK brings anyway.
///
/// - `PrepareForSleep(true)`, just before a sleep: release everything. No delay inhibitor is
///   taken, so the release may only go out once the machine is awake again; a frozen process
///   types nothing meanwhile.
/// - `PrepareForSleep(false)`, after waking: reopen the MIDI ports.
/// - This session's `Lock` and `Unlock` (`loginctl lock-session` and the like), and its
///   `LockedHint` turning true or false (desktops that lock on their own, GNOME on idle or
///   Super+L, only report that): release, and reopen. A locker logind never hears of (a bare
///   i3lock started on X idle) is not seen.
///
/// `Active` is not read: a wrong value there would make the app look dead. Any failure (no
/// system bus, no logind, no session) is logged once and leaves the app as it was; logind is
/// never started on the app's account.
#[cfg(target_os = "linux")]
mod imp {
    use std::cell::Cell;
    use std::collections::HashMap;

    use gio::glib;
    use gio::prelude::*;

    use super::shared::{Change, apply};

    const LOGIN1: &str = "org.freedesktop.login1";
    const MANAGER_PATH: &str = "/org/freedesktop/login1";
    const MANAGER: &str = "org.freedesktop.login1.Manager";
    const SESSION: &str = "org.freedesktop.login1.Session";
    const PROPERTIES: &str = "org.freedesktop.DBus.Properties";
    /// For the calls made at startup (on this thread, so nothing else waits for them).
    const CALL_TIMEOUT_MS: i32 = 10_000;

    /// The watcher thread: the subscriptions are served by a main context of its own.
    pub(super) fn run() {
        let context = glib::MainContext::new();
        let main_loop = glib::MainLoop::new(Some(&context), false);
        let entered = context.with_thread_default(|| {
            // Kept for as long as the subscriptions are wanted: forever.
            if let Some(_bus) = subscribe() {
                main_loop.run();
            }
        });
        if let Err(error) = entered {
            log::warn!("not watching for sleep and lock: {error}");
        }
    }

    /// Subscribes to sleep and to this session's lock; `None` when there is nothing to watch.
    fn subscribe() -> Option<gio::DBusConnection> {
        let bus = match gio::bus_get_sync(gio::BusType::System, gio::Cancellable::NONE) {
            Ok(bus) => bus,
            Err(error) => {
                log::warn!("not watching for sleep and lock: no system bus ({error})");
                return None;
            }
        };
        bus.signal_subscribe(
            Some(LOGIN1),
            Some(MANAGER),
            Some("PrepareForSleep"),
            Some(MANAGER_PATH),
            None,
            gio::DBusSignalFlags::NONE,
            |_, _, _, _, _, parameters| {
                if let Some((sleeping,)) = parameters.get::<(bool,)>() {
                    apply(if sleeping { Change::Away } else { Change::Back });
                }
            },
        );
        let path = match session_path(&bus) {
            Ok(path) => path,
            Err(error) => {
                log::warn!("not watching for the screen lock: {error}");
                return Some(bus);
            }
        };
        for (signal, change) in [("Lock", Change::Away), ("Unlock", Change::Back)] {
            bus.signal_subscribe(
                Some(LOGIN1),
                Some(SESSION),
                Some(signal),
                Some(&path),
                None,
                gio::DBusSignalFlags::NONE,
                move |_, _, _, _, _, _| apply(change),
            );
        }
        // Acts when the hint changes, not on every report of it.
        let locked = Cell::new(None::<bool>);
        bus.signal_subscribe(
            Some(LOGIN1),
            Some(PROPERTIES),
            Some("PropertiesChanged"),
            Some(&path),
            Some(SESSION),
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, parameters| {
                let Some((_, changed, _)) =
                    parameters.get::<(String, HashMap<String, glib::Variant>, Vec<String>)>()
                else {
                    return;
                };
                let Some(now) = changed.get("LockedHint").and_then(|hint| hint.get::<bool>())
                else {
                    return;
                };
                if locked.replace(Some(now)) != Some(now) {
                    apply(if now { Change::Away } else { Change::Back });
                }
            },
        );
        Some(bus)
    }

    /// The object path of this app's login session: its own, or else its user's graphical one
    /// ("auto"; an app started as a user service is in no session of its own). Older logind
    /// versions only know a process's own session.
    fn session_path(bus: &gio::DBusConnection) -> Result<String, String> {
        let reply_type = glib::VariantTy::new("(o)").map_err(|error| error.to_string())?;
        let call = |method: &str, arguments: glib::Variant| {
            bus.call_sync(
                Some(LOGIN1),
                MANAGER_PATH,
                MANAGER,
                method,
                Some(&arguments),
                Some(reply_type),
                gio::DBusCallFlags::NO_AUTO_START,
                CALL_TIMEOUT_MS,
                gio::Cancellable::NONE,
            )
        };
        let reply = call("GetSession", ("auto",).to_variant())
            .or_else(|_| call("GetSessionByPID", (std::process::id(),).to_variant()))
            .map_err(|error| format!("no login session ({error})"))?;
        reply
            .try_child_value(0)
            .and_then(|path| path.str().map(str::to_owned))
            .ok_or_else(|| "logind answered without a session path".to_owned())
    }
}
