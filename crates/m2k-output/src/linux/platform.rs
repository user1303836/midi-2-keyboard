//! Linux platform probes.
//!
//! X11: the active window from `_NET_ACTIVE_WINDOW` (EWMH), its `WM_CLASS` (`app_id`, the
//! class part) and `_NET_WM_NAME` / `WM_NAME`. The process is the one the X server knows owns
//! the window (X-Resource extension); `_NET_WM_PID`, which the application reports itself and
//! which is wrong for sandboxed (Flatpak) applications, only without that extension. Its name
//! is the executable's (`/proc/<pid>/exe`), except for interpreters, runtimes and Wine, where the
//! window class names the application. `foreground_app` and `running_apps` describe a window
//! the same way, so a rule picked from the list matches when that application is in front.
//! One connection is kept open between polls and re-established when it breaks.
//! Wayland gives clients no way to see the focused window, so nothing is reported there.

use std::collections::HashSet;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
use x11rb::protocol::res::{ClientIdMask, ClientIdSpec, ConnectionExt as _, QueryClientIdsReply};
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, GetPropertyReply, Window};
use x11rb::rust_connection::RustConnection;

use super::{is_wayland, uinput};
use crate::platform::{ForegroundApp, Permissions};

const WAYLAND_FOREGROUND_HINT: &str = "Wayland does not let applications see which window has \
     focus, so profiles and conditions that depend on the active application cannot switch \
     automatically.";

/// How long to wait before trying to reach the X server again after it failed.
const RETRY: Duration = Duration::from_secs(5);

pub(crate) fn foreground_app() -> Option<ForegroundApp> {
    if is_wayland() {
        return None;
    }
    with_x(|x| match x.active_window()? {
        Some(window) => x.describe(window),
        None => Ok(None),
    })
    .flatten()
}

pub(crate) fn running_apps() -> Vec<ForegroundApp> {
    if is_wayland() {
        return Vec::new();
    }
    with_x(|x| x.client_apps()).unwrap_or_default()
}

pub(crate) fn permissions() -> Permissions {
    let access = uinput::access();
    let uinput_ok = matches!(access, uinput::Access::Ok);
    if is_wayland() {
        let hint = if uinput_ok {
            WAYLAND_FOREGROUND_HINT.to_string()
        } else {
            format!(
                "On Wayland, midi-2-keyboard sends input through /dev/uinput; until that works \
                 only X11 (XWayland) applications receive it. {} {WAYLAND_FOREGROUND_HINT}",
                uinput::access_hint(&access)
            )
        };
        return Permissions { can_inject: uinput_ok, can_detect_foreground: false, hint };
    }
    let x11_ok = with_x(|_| Ok(())).is_some();
    match (x11_ok, uinput_ok) {
        (true, _) => {
            Permissions { can_inject: true, can_detect_foreground: true, hint: String::new() }
        }
        (false, true) => Permissions {
            can_inject: true,
            can_detect_foreground: false,
            hint: "No X server is reachable (DISPLAY not set?), so the active application \
                   cannot be detected."
                .into(),
        },
        (false, false) => Permissions {
            can_inject: false,
            can_detect_foreground: false,
            hint: format!(
                "No X server is reachable (DISPLAY not set?) and /dev/uinput is not usable. {}",
                uinput::access_hint(&access)
            ),
        },
    }
}

pub(crate) fn request_permissions() {}

/// Linux has no privilege separation between applications' input (no UIPI).
pub(crate) fn is_elevated() -> bool {
    false
}

pub(crate) fn relaunch_elevated(_args: &[String]) -> Result<(), String> {
    Err(crate::platform::NO_ELEVATION.into())
}

/// The connection broke: drop it and reconnect later.
struct Broken;

type XResult<T> = std::result::Result<T, Broken>;

struct Probe {
    conn: Option<XConn>,
    retry_at: Option<Instant>,
}

static PROBE: Mutex<Probe> = Mutex::new(Probe { conn: None, retry_at: None });

/// Runs `f` with the shared X connection, connecting first when needed. `None` when no X server
/// is reachable or the connection broke during `f`.
fn with_x<T>(f: impl FnOnce(&XConn) -> XResult<T>) -> Option<T> {
    let mut probe = PROBE.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if probe.conn.is_none() {
        if probe.retry_at.is_some_and(|at| Instant::now() < at) {
            return None;
        }
        match XConn::connect() {
            Some(conn) => probe.conn = Some(conn),
            None => {
                probe.retry_at = Some(Instant::now() + RETRY);
                return None;
            }
        }
    }
    let result = f(probe.conn.as_ref()?);
    match result {
        Ok(value) => Some(value),
        Err(Broken) => {
            probe.conn = None;
            probe.retry_at = Some(Instant::now() + Duration::from_secs(1));
            None
        }
    }
}

struct XConn {
    conn: RustConnection,
    root: Window,
    net_active_window: u32,
    net_wm_name: u32,
    net_wm_pid: u32,
    utf8_string: u32,
    net_client_list: u32,
    net_wm_window_type: u32,
    type_dock: u32,
    type_desktop: u32,
    net_wm_state: u32,
    state_skip_taskbar: u32,
    /// X-Resource 1.2 (which client process owns a window) is available.
    xres: bool,
}

impl XConn {
    fn connect() -> Option<Self> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let xres = conn
            .res_query_version(1, 2)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|version| (version.server_major, version.server_minor) >= (1, 2));
        let names: [&[u8]; 10] = [
            b"_NET_ACTIVE_WINDOW",
            b"_NET_WM_NAME",
            b"_NET_WM_PID",
            b"UTF8_STRING",
            b"_NET_CLIENT_LIST",
            b"_NET_WM_WINDOW_TYPE",
            b"_NET_WM_WINDOW_TYPE_DOCK",
            b"_NET_WM_WINDOW_TYPE_DESKTOP",
            b"_NET_WM_STATE",
            b"_NET_WM_STATE_SKIP_TASKBAR",
        ];
        // Send all requests before waiting for the first reply.
        let atoms = names
            .iter()
            .map(|name| conn.intern_atom(false, name).ok())
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .map(|cookie| cookie.reply().ok().map(|reply| reply.atom))
            .collect::<Option<Vec<_>>>()?;
        let [
            net_active_window,
            net_wm_name,
            net_wm_pid,
            utf8_string,
            net_client_list,
            net_wm_window_type,
            type_dock,
            type_desktop,
            net_wm_state,
            state_skip_taskbar,
        ] = atoms[..]
        else {
            return None;
        };
        Some(Self {
            conn,
            root,
            net_active_window,
            net_wm_name,
            net_wm_pid,
            utf8_string,
            net_client_list,
            net_wm_window_type,
            type_dock,
            type_desktop,
            net_wm_state,
            state_skip_taskbar,
            xres,
        })
    }

    fn property(
        &self,
        window: Window,
        property: impl Into<u32>,
        kind: impl Into<u32>,
        len: u32,
    ) -> XResult<Option<GetPropertyReply>> {
        let cookie = self
            .conn
            .get_property(false, window, property.into(), kind.into(), 0, len)
            .map_err(|_| Broken)?;
        reply(cookie.reply())
    }

    fn active_window(&self) -> XResult<Option<Window>> {
        if let Some(reply) =
            self.property(self.root, self.net_active_window, AtomEnum::WINDOW, 1)?
            && reply.type_ != 0
        {
            let window = reply.value32().and_then(|mut v| v.next()).unwrap_or(0);
            return Ok((window != 0).then_some(window));
        }
        // No EWMH window manager: fall back to the input focus.
        let focus = reply(self.conn.get_input_focus().map_err(|_| Broken)?.reply())?;
        Ok(focus.map(|f| f.focus).filter(|&w| w > 1)) // 0 = None, 1 = PointerRoot
    }

    /// Applications of the managed windows, one per process, sorted by name.
    fn client_apps(&self) -> XResult<Vec<ForegroundApp>> {
        let windows: Vec<Window> = self
            .property(self.root, self.net_client_list, AtomEnum::WINDOW, 4096)?
            .and_then(|reply| reply.value32().map(|values| values.collect()))
            .unwrap_or_default();
        let mut seen = HashSet::new();
        let mut apps = Vec::new();
        for window in windows {
            if self.hidden_from_taskbar(window)? {
                continue;
            }
            let Some(app) = self.describe(window)? else { continue };
            let key = if app.process.is_empty() { &app.app_id } else { &app.process };
            if key.is_empty() || !seen.insert(key.to_lowercase()) {
                continue;
            }
            apps.push(app);
        }
        apps.sort_by_cached_key(|app| {
            if app.process.is_empty() {
                app.app_id.to_lowercase()
            } else {
                app.process.to_lowercase()
            }
        });
        Ok(apps)
    }

    /// Docks, desktops and windows that ask to be left off the taskbar.
    fn hidden_from_taskbar(&self, window: Window) -> XResult<bool> {
        let kind =
            self.conn.get_property(false, window, self.net_wm_window_type, AtomEnum::ATOM, 0, 32);
        let state = self.conn.get_property(false, window, self.net_wm_state, AtomEnum::ATOM, 0, 32);
        let kind = reply(kind.map_err(|_| Broken)?.reply())?;
        let state = reply(state.map_err(|_| Broken)?.reply())?;
        let has = |reply: &Option<GetPropertyReply>, atom: u32| {
            reply
                .as_ref()
                .and_then(|r| r.value32())
                .is_some_and(|mut values| values.any(|a| a == atom))
        };
        Ok(has(&kind, self.type_dock)
            || has(&kind, self.type_desktop)
            || has(&state, self.state_skip_taskbar))
    }

    fn describe(&self, window: Window) -> XResult<Option<ForegroundApp>> {
        // Send all requests, then collect the replies (one round trip).
        let class =
            self.conn.get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256);
        let name =
            self.conn.get_property(false, window, self.net_wm_name, self.utf8_string, 0, 1024);
        let pid = self.conn.get_property(false, window, self.net_wm_pid, AtomEnum::CARDINAL, 0, 1);
        let owner = self.xres.then(|| {
            let spec = ClientIdSpec { client: window, mask: ClientIdMask::LOCAL_CLIENT_PID };
            self.conn.res_query_client_ids(&[spec])
        });
        let class = reply(class.map_err(|_| Broken)?.reply())?;
        let name = reply(name.map_err(|_| Broken)?.reply())?;
        let pid = reply(pid.map_err(|_| Broken)?.reply())?;
        let owner = match owner {
            Some(cookie) => Some(reply(cookie.map_err(|_| Broken)?.reply())?),
            None => None,
        };
        if class.is_none() && name.is_none() {
            return Ok(None); // the window is gone
        }
        let mut title =
            name.map(|r| String::from_utf8_lossy(&r.value).into_owned()).unwrap_or_default();
        if title.is_empty()
            && let Some(legacy) = self.property(window, AtomEnum::WM_NAME, AtomEnum::ANY, 1024)?
        {
            title = decode_text(&legacy.value);
        }
        let mut parts = class
            .map(|r| r.value)
            .unwrap_or_default()
            .split(|&b| b == 0)
            .map(|part| String::from_utf8_lossy(part).into_owned())
            .collect::<Vec<_>>()
            .into_iter();
        let instance = parts.next().unwrap_or_default();
        let class_name = parts.next().filter(|c| !c.is_empty()).unwrap_or(instance);
        let pid = match owner {
            // The process the X server sees on the other end of the window's connection, in our
            // PID namespace. None: a remote client (its _NET_WM_PID would name some unrelated
            // local process) or one the server cannot identify.
            Some(owner) => owner.as_ref().map_or(0, local_pid),
            None => pid.and_then(|r| r.value32().and_then(|mut v| v.next())).unwrap_or(0),
        };
        let mut process = process_name(pid);
        if is_host(&process) && !class_name.is_empty() {
            process = class_name.clone();
        }
        Ok(Some(ForegroundApp { process, title, app_id: class_name, pid, elevated: false }))
    }
}

/// The local process id in an X-Resource client id reply (0 when there is none).
fn local_pid(reply: &QueryClientIdsReply) -> u32 {
    reply
        .ids
        .iter()
        .find(|id| u32::from(id.spec.mask) & u32::from(ClientIdMask::LOCAL_CLIENT_PID) != 0)
        .and_then(|id| id.value.first().copied())
        .unwrap_or(0)
}

/// Programs that run an application rather than being one (interpreters, runtimes, Wine): their
/// executable name says nothing about the application in the window.
fn is_host(process: &str) -> bool {
    // python3.12 -> python, electron29 -> electron, wine64 -> wine
    let name = process.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    matches!(
        name,
        "python"
            | "pypy"
            | "java"
            | "node"
            | "nodejs"
            | "electron"
            | "perl"
            | "ruby"
            | "mono"
            | "mono-sgen"
            | "dotnet"
            | "gjs"
            | "gjs-console"
            | "lua"
            | "luajit"
            | "php"
            | "tclsh"
            | "wish"
            | "wine"
            | "wine-preloader"
            | "wine64-preloader"
    )
}

/// A reply, `None` for X protocol errors (e.g. the window vanished), `Broken` when the
/// connection failed.
fn reply<T>(result: std::result::Result<T, ReplyError>) -> XResult<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(ReplyError::X11Error(_)) => Ok(None),
        Err(ReplyError::ConnectionError(_)) => Err(Broken),
    }
}

/// WM_NAME is Latin-1 (STRING) or, from most modern clients, UTF-8.
fn decode_text(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&b| char::from(b)).collect(),
    }
}

/// Executable name of `pid` (`/proc/<pid>/exe`, else the 15-character `comm`).
fn process_name(pid: u32) -> String {
    if pid == 0 {
        return String::new();
    }
    if let Ok(path) = std::fs::read_link(format!("/proc/{pid}/exe"))
        && let Some(name) = path.file_name()
    {
        let name = name.to_string_lossy();
        return name.strip_suffix(" (deleted)").unwrap_or(&name).to_string();
    }
    std::fs::read_to_string(format!("/proc/{pid}/comm"))
        .map(|comm| comm.trim_end().to_string())
        .unwrap_or_default()
}
