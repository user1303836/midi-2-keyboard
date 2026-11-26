//! Windows platform probes.

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS, GetLastError, HANDLE, HWND, LPARAM,
};
use windows_sys::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetForegroundWindow, GetWindowThreadProcessId, InternalGetWindowText,
};
use windows_sys::core::BOOL;

use crate::platform::{ForegroundApp, Permissions};

/// Host process of UWP / WinUI app windows; the real app owns a child window.
const FRAME_HOST: &str = "ApplicationFrameHost.exe";

pub(crate) fn foreground_app() -> Option<ForegroundApp> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_null() {
        return None;
    }
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
    Some(ForegroundApp { process: info.name, title: window_title(hwnd), app_id: info.app_id, pid })
}

pub(crate) fn permissions() -> Permissions {
    // SendInput needs no permission. Elevated target windows (UIPI) are reported per event.
    Permissions { can_inject: true, can_detect_foreground: true, hint: String::new() }
}

pub(crate) fn request_permissions() {}

/// Executable name of the process owning the foreground window (for error messages).
pub(crate) fn foreground_process_name() -> Option<String> {
    foreground_app().map(|app| app.process)
}

#[derive(Default)]
struct ProcessInfo {
    /// Executable file name, e.g. `notepad.exe`.
    name: String,
    /// AppUserModelID of packaged (Store / MSIX) apps, empty otherwise.
    app_id: String,
}

fn process_info(pid: u32) -> Option<ProcessInfo> {
    // PROCESS_QUERY_LIMITED_INFORMATION is granted even for elevated processes; it fails for a
    // few protected system processes only.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return None;
    }
    let info = ProcessInfo {
        name: image_file_name(handle).unwrap_or_default(),
        app_id: app_user_model_id(handle).unwrap_or_default(),
    };
    unsafe { CloseHandle(handle) };
    Some(info)
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
