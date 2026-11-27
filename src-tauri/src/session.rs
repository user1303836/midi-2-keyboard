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
//! Not watched elsewhere yet. macOS drops input posted from a session in the background (fast
//! user switching) without an error; on Linux the lock screen receives XTest and uinput input,
//! and uinput reaches whichever session is in front.

#[cfg(windows)]
pub use imp::watch;

#[cfg(not(windows))]
pub fn watch(_app: tauri::AppHandle) {}

#[cfg(windows)]
mod imp {
    use std::mem::zeroed;
    use std::ptr::{null, null_mut};
    use std::sync::OnceLock;

    use tauri::{AppHandle, Manager};
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

    use crate::AppState;

    static APP: OnceLock<AppHandle> = OnceLock::new();

    enum Change {
        /// Sleep, lock, switched away.
        Away,
        /// Resume, unlock, switched back.
        Back,
    }

    /// Starts watching (once), on a thread with a window of its own.
    pub fn watch(app: AppHandle) {
        if APP.set(app).is_err() {
            return;
        }
        let spawned = std::thread::Builder::new().name("m2k-session-watch".into()).spawn(|| {
            // SAFETY: a window class and a window owned by this thread, and its message loop.
            unsafe { run() }
        });
        if let Err(error) = spawned {
            log::warn!("could not watch for sleep and lock: {error}");
        }
    }

    unsafe fn run() {
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
            // Nothing may unwind into the OS.
            let _ = std::panic::catch_unwind(|| apply(change));
            if message == WM_POWERBROADCAST {
                return 1;
            }
        }
        unsafe { DefWindowProcW(window, message, wparam, lparam) }
    }

    fn apply(change: Change) {
        let Some(state) = APP.get().and_then(|app| app.try_state::<AppState>()) else { return };
        match change {
            Change::Away => state.runtime.release_all(),
            Change::Back => state.runtime.reopen_midi(),
        }
    }
}
