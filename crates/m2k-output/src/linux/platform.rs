//! Linux platform probes.
//!
//! X11: the active window from `_NET_ACTIVE_WINDOW` (EWMH), its `WM_CLASS` (`app_id`, the
//! class part), `_NET_WM_NAME` / `WM_NAME` and `_NET_WM_PID` (+ `/proc/<pid>/exe` for the
//! process name). One connection is kept open between polls and re-established when it breaks.
//! Wayland gives clients no way to see the focused window, so nothing is reported there.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::errors::ReplyError;
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
}

impl XConn {
    fn connect() -> Option<Self> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots.get(screen)?.root;
        let names: [&[u8]; 4] =
            [b"_NET_ACTIVE_WINDOW", b"_NET_WM_NAME", b"_NET_WM_PID", b"UTF8_STRING"];
        // Send all four requests before waiting for the first reply.
        let atoms = names
            .iter()
            .map(|name| conn.intern_atom(false, name).ok())
            .collect::<Option<Vec<_>>>()?
            .into_iter()
            .map(|cookie| cookie.reply().ok().map(|reply| reply.atom))
            .collect::<Option<Vec<_>>>()?;
        let [net_active_window, net_wm_name, net_wm_pid, utf8_string] = atoms[..] else {
            return None;
        };
        Some(Self { conn, root, net_active_window, net_wm_name, net_wm_pid, utf8_string })
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

    fn describe(&self, window: Window) -> XResult<Option<ForegroundApp>> {
        // Send all requests, then collect the replies (one round trip).
        let class =
            self.conn.get_property(false, window, AtomEnum::WM_CLASS, AtomEnum::STRING, 0, 256);
        let name =
            self.conn.get_property(false, window, self.net_wm_name, self.utf8_string, 0, 1024);
        let pid = self.conn.get_property(false, window, self.net_wm_pid, AtomEnum::CARDINAL, 0, 1);
        let class = reply(class.map_err(|_| Broken)?.reply())?;
        let name = reply(name.map_err(|_| Broken)?.reply())?;
        let pid = reply(pid.map_err(|_| Broken)?.reply())?;
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
        let pid = pid.and_then(|r| r.value32().and_then(|mut v| v.next())).unwrap_or(0);
        Ok(Some(ForegroundApp { process: process_name(pid), title, app_id: class_name, pid }))
    }
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
