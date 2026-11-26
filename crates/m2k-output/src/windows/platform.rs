//! Windows platform probes.

use std::collections::HashSet;
use std::ptr::null_mut;
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GetLastError,
    HANDLE, HWND, LPARAM,
};
use windows_sys::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DwmGetWindowAttribute};
use windows_sys::Win32::Security::{
    GetSidSubAuthority, GetSidSubAuthorityCount, GetTokenInformation, TOKEN_ELEVATION,
    TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenElevation, TokenIntegrityLevel,
};
use windows_sys::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GW_OWNER, GWL_EXSTYLE, GetForegroundWindow, GetWindow,
    GetWindowLongW, GetWindowThreadProcessId, InternalGetWindowText, IsWindowVisible,
    WS_EX_TOOLWINDOW,
};
use windows_sys::core::BOOL;

use crate::platform::{ForegroundApp, Permissions};

/// Host process of UWP / WinUI app windows; the real app owns a child window.
const FRAME_HOST: &str = "ApplicationFrameHost.exe";

/// Mandatory integrity levels (the last sub-authority of the label SID).
const MEDIUM_INTEGRITY: u32 = 0x2000;
const HIGH_INTEGRITY: u32 = 0x3000;

pub(crate) fn foreground_app() -> Option<ForegroundApp> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
    let (pid, info) = window_process(hwnd)?;
    Some(ForegroundApp {
        process: info.name,
        title: window_title(hwnd),
        app_id: info.app_id,
        pid,
        elevated: info.elevated,
    })
}

pub(crate) fn running_apps() -> Vec<ForegroundApp> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the Vec below, alive for the whole enumeration.
        let windows = unsafe { &mut *(lparam as *mut Vec<HWND>) };
        windows.push(hwnd);
        1
    }
    let mut windows: Vec<HWND> = Vec::new();
    unsafe { EnumWindows(Some(collect), &mut windows as *mut Vec<HWND> as LPARAM) };
    let mut seen = HashSet::new();
    let mut apps = Vec::new();
    for hwnd in windows {
        if !is_app_window(hwnd) {
            continue;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        let Some(mut info) = process_info(pid) else { continue };
        if info.name.eq_ignore_ascii_case(FRAME_HOST) {
            // A frame without hosted content belongs to a suspended app: nothing to show.
            match hosted_app_pid(hwnd, pid).and_then(|p| process_info(p).map(|i| (p, i))) {
                Some((app_pid, app)) => (pid, info) = (app_pid, app),
                None => continue,
            }
        }
        if info.name.is_empty() || !seen.insert(info.name.to_lowercase()) {
            continue;
        }
        apps.push(ForegroundApp {
            process: info.name,
            title: window_title(hwnd),
            app_id: info.app_id,
            pid,
            elevated: info.elevated,
        });
    }
    apps.sort_by_cached_key(|app| app.process.to_lowercase());
    apps
}

/// A window the taskbar / Alt+Tab would show: visible, titled, not owned by another window,
/// not a tool window and not cloaked (background UWP apps, other virtual desktops).
fn is_app_window(hwnd: HWND) -> bool {
    unsafe {
        if IsWindowVisible(hwnd) == 0 || !GetWindow(hwnd, GW_OWNER).is_null() {
            return false;
        }
        if GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW != 0 {
            return false;
        }
        let mut cloaked = 0u32;
        let result = DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED as u32,
            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
            size_of::<u32>() as u32,
        );
        if result >= 0 && cloaked != 0 {
            return false;
        }
    }
    !window_title(hwnd).is_empty()
}

pub(crate) fn permissions() -> Permissions {
    // SendInput needs no permission. Elevated target windows (UIPI) are reported per event and
    // through `ForegroundApp::elevated`.
    Permissions { can_inject: true, can_detect_foreground: true, hint: String::new() }
}

pub(crate) fn request_permissions() {}

/// Whether this process runs elevated (as administrator).
pub(crate) fn is_elevated() -> bool {
    let mut token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return own_integrity() >= HIGH_INTEGRITY;
    }
    let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut len = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenElevation,
            (&mut elevation as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut len,
        )
    };
    unsafe { CloseHandle(token) };
    if ok != 0 { elevation.TokenIsElevated != 0 } else { own_integrity() >= HIGH_INTEGRITY }
}

/// When the process owning `hwnd` runs at a higher integrity level than we do (so Windows keeps
/// our input out of the window): its executable name, possibly empty. `None` otherwise.
pub(crate) fn elevated_owner(hwnd: HWND) -> Option<String> {
    if hwnd.is_null() {
        return None;
    }
    let (_, info) = window_process(hwnd)?;
    info.elevated.then_some(info.name)
}

/// The process behind a window, with UWP frames resolved to the hosted app.
fn window_process(hwnd: HWND) -> Option<(u32, ProcessInfo)> {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
    if pid == 0 {
        return None;
    }
    let mut info = process_info(pid).unwrap_or_default();
    if info.name.eq_ignore_ascii_case(FRAME_HOST)
        && let Some(app_pid) = hosted_app_pid(hwnd, pid)
        && let Some(app) = process_info(app_pid)
    {
        pid = app_pid;
        info = app;
    }
    Some((pid, info))
}

#[derive(Default)]
struct ProcessInfo {
    /// Executable file name, e.g. `notepad.exe`.
    name: String,
    /// AppUserModelID of packaged (Store / MSIX) apps, empty otherwise.
    app_id: String,
    /// Runs at a higher integrity level than this process (UIPI blocks our input into it).
    elevated: bool,
}

fn process_info(pid: u32) -> Option<ProcessInfo> {
    // PROCESS_QUERY_LIMITED_INFORMATION is granted even for elevated processes; it fails for a
    // few protected system processes only.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        // A process we may not even look at is more privileged than we are.
        let denied = unsafe { GetLastError() } == ERROR_ACCESS_DENIED;
        return denied.then(|| ProcessInfo { elevated: true, ..ProcessInfo::default() });
    }
    let info = ProcessInfo {
        name: image_file_name(handle).unwrap_or_default(),
        app_id: app_user_model_id(handle).unwrap_or_default(),
        elevated: match integrity_level(handle) {
            Ok(level) => level > own_integrity(),
            Err(error) => error == ERROR_ACCESS_DENIED,
        },
    };
    unsafe { CloseHandle(handle) };
    Some(info)
}

/// The mandatory integrity level of a process (`0x2000` medium, `0x3000` high = administrator,
/// `0x4000` system), or the Win32 error that prevented reading it.
fn integrity_level(process: HANDLE) -> Result<u32, u32> {
    let mut token: HANDLE = null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(unsafe { GetLastError() });
    }
    // TOKEN_MANDATORY_LABEL followed by the SID it points to; u64 for alignment.
    let mut buffer = [0u64; 16];
    let mut len = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token,
            TokenIntegrityLevel,
            buffer.as_mut_ptr().cast(),
            size_of_val(&buffer) as u32,
            &mut len,
        )
    };
    let error = unsafe { GetLastError() };
    unsafe { CloseHandle(token) };
    if ok == 0 {
        return Err(error);
    }
    // SAFETY: on success the buffer starts with a TOKEN_MANDATORY_LABEL whose SID lies in it.
    unsafe {
        let label = &*(buffer.as_ptr() as *const TOKEN_MANDATORY_LABEL);
        let sid = label.Label.Sid;
        let count = *GetSidSubAuthorityCount(sid);
        if count == 0 {
            return Err(0);
        }
        Ok(*GetSidSubAuthority(sid, u32::from(count) - 1))
    }
}

/// Our own integrity level, read once.
fn own_integrity() -> u32 {
    static LEVEL: OnceLock<u32> = OnceLock::new();
    *LEVEL
        .get_or_init(|| integrity_level(unsafe { GetCurrentProcess() }).unwrap_or(MEDIUM_INTEGRITY))
}

fn image_file_name(process: HANDLE) -> Option<String> {
    let mut buffer = vec![0u16; 1024];
    loop {
        let mut len = buffer.len() as u32;
        let ok = unsafe {
            QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, buffer.as_mut_ptr(), &mut len)
        };
        if ok != 0 {
            let path = String::from_utf16_lossy(&buffer[..len as usize]);
            let name = path.rsplit(['\\', '/']).next().unwrap_or(&path);
            return Some(name.to_string());
        }
        if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || buffer.len() >= 32 * 1024 {
            return None;
        }
        buffer.resize(32 * 1024, 0);
    }
}

fn app_user_model_id(process: HANDLE) -> Option<String> {
    // APPLICATION_USER_MODEL_ID_MAX_LENGTH
    let mut buffer = [0u16; 130];
    let mut len = buffer.len() as u32;
    let status = unsafe { GetApplicationUserModelId(process, &mut len, buffer.as_mut_ptr()) };
    if status != ERROR_SUCCESS {
        return None; // APPMODEL_ERROR_NO_APPLICATION for unpackaged processes
    }
    let end = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
}

fn window_title(hwnd: HWND) -> String {
    // InternalGetWindowText reads the caption directly. GetWindowText would send WM_GETTEXT
    // to windows of our own process and could block on a busy UI thread.
    let mut buffer = [0u16; 512];
    let len = unsafe { InternalGetWindowText(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
    String::from_utf16_lossy(&buffer[..len.clamp(0, buffer.len() as i32) as usize])
}

/// For an ApplicationFrameHost frame: the process that owns its content (a child window from
/// another process).
fn hosted_app_pid(frame: HWND, frame_pid: u32) -> Option<u32> {
    struct Search {
        frame_pid: u32,
        found: u32,
    }

    unsafe extern "system" fn visit(child: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Search` below, alive for the whole enumeration.
        let search = unsafe { &mut *(lparam as *mut Search) };
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(child, &mut pid) };
        if pid != 0 && pid != search.frame_pid {
            search.found = pid;
            return 0; // stop
        }
        1
    }

    let mut search = Search { frame_pid, found: 0 };
    unsafe { EnumChildWindows(frame, Some(visit), &mut search as *mut Search as LPARAM) };
    (search.found != 0).then_some(search.found)
}
