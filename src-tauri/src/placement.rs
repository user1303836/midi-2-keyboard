//! Where the main window was: its size, position and whether it was maximized.
//!
//! Kept in memory while the app runs, so a reopen from the tray puts the window back where it
//! was, and saved in `window.json` next to the config for the next start (machine state, not
//! configuration). Captured on every move and resize of the main window (minimized states are
//! ignored; while maximized, the bounds it returns to are kept), written half a second after it
//! settles and again at exit.
//!
//! A saved placement is only used if it still fits a monitor that is connected now with the same
//! scaling: within the size of its work area, at least half on it and with the title bar on it.
//! Otherwise (the second screen was unplugged, the resolution or the scaling changed) the window
//! gets its default size, centered. It is never smaller than its minimum size, and a window that
//! was maximized comes back maximized, with its normal bounds underneath.

use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{Monitor, PhysicalPosition, PhysicalSize, Runtime, WebviewWindow, Window};

/// Quiet time before a changed placement is written (a drag sends dozens of moves a second).
const SETTLE: Duration = Duration::from_millis(500);

/// The main window's placement, in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowPlacement {
    /// Outer position and inner size of the window when it is not maximized.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    /// Scale factor of the monitor it was on.
    pub scale: f64,
    pub maximized: bool,
}

pub struct Placement {
    path: PathBuf,
    state: Mutex<State>,
    wake: Condvar,
}

#[derive(Default)]
struct State {
    current: Option<WindowPlacement>,
    written: Option<WindowPlacement>,
    changed_at: Option<Instant>,
}

impl Placement {
    /// Reads `path` (missing or unreadable: no placement) and starts the writer thread.
    pub fn load(path: PathBuf) -> Arc<Placement> {
        let saved = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<WindowPlacement>(&text).ok())
            .filter(|p| p.width > 0 && p.height > 0 && p.scale.is_finite() && p.scale > 0.0);
        let placement = Arc::new(Placement {
            path,
            state: Mutex::new(State { current: saved, written: saved, changed_at: None }),
            wake: Condvar::new(),
        });
        let writer = Arc::clone(&placement);
        let spawned = std::thread::Builder::new()
            .name("m2k-window-placement".into())
            .spawn(move || writer.run());
        if let Err(error) = spawned {
            log::warn!("could not start the window placement writer: {error}");
        }
        placement
    }

    fn current(&self) -> Option<WindowPlacement> {
        self.lock().current
    }

    fn update(&self, placement: WindowPlacement) {
        let mut state = self.lock();
        if state.current != Some(placement) {
            state.current = Some(placement);
            state.changed_at = Some(Instant::now());
            self.wake.notify_one();
        }
    }

    /// Writes the latest placement now unless it already is on disk (at exit).
    pub fn flush(&self) {
        let pending = {
            let state = self.lock();
            if state.current == state.written { None } else { state.current }
        };
        if let Some(placement) = pending {
            self.write(&placement);
            self.lock().written = Some(placement);
        }
    }

    fn run(&self) {
        let mut state = self.lock();
        loop {
            while state.current == state.written {
                state = self.wake.wait(state).unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            // Let a drag or a resize finish first.
            while let Some(since) = state.changed_at.map(|at| at.elapsed())
                && since < SETTLE
            {
                state = self
                    .wake
                    .wait_timeout(state, SETTLE - since)
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .0;
            }
            let Some(placement) = state.current else { continue };
            drop(state);
            self.write(&placement);
            state = self.lock();
            // Also after a failed write: the next change tries again.
            state.written = Some(placement);
        }
    }

    fn write(&self, placement: &WindowPlacement) {
        let Ok(text) = serde_json::to_string_pretty(placement) else { return };
        let temporary = self.path.with_extension("json.tmp");
        let written = std::fs::write(&temporary, text + "\n")
            .and_then(|()| std::fs::rename(&temporary, &self.path));
        if let Err(error) = written {
            log::warn!("could not save the window placement to {}: {error}", self.path.display());
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// Whether the window is minimized / maximized right now.
///
/// Windows moves a window before it resizes it, and tao updates its own flags only with the
/// resize: during a maximize the move would still read "not maximized" (and a minimize would
/// record -32000, -32000). The OS itself already knows when the move arrives.
#[cfg(windows)]
fn window_state<R: Runtime>(window: &Window<R>) -> Option<(bool, bool)> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsZoomed};
    let hwnd = window.hwnd().ok()?.0;
    // SAFETY: plain queries on the window's own handle.
    unsafe { Some((IsIconic(hwnd) != 0, IsZoomed(hwnd) != 0)) }
}

#[cfg(not(windows))]
fn window_state<R: Runtime>(window: &Window<R>) -> Option<(bool, bool)> {
    Some((window.is_minimized().ok()?, window.is_maximized().ok()?))
}

/// The main window moved or changed size: remember where it is.
pub fn remember<R: Runtime>(window: &Window<R>, placement: &Placement) {
    let Some((minimized, maximized)) = window_state(window) else { return };
    if minimized {
        return;
    }
    let next = if maximized {
        // Keep the bounds it returns to.
        let Some(normal) = placement.current() else { return };
        WindowPlacement { maximized: true, ..normal }
    } else {
        let (Ok(position), Ok(size), Ok(scale)) =
            (window.outer_position(), window.inner_size(), window.scale_factor())
        else {
            return;
        };
        if size.width == 0 || size.height == 0 {
            return;
        }
        WindowPlacement {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            scale,
            maximized: false,
        }
    };
    placement.update(next);
}

/// Puts a newly created, still hidden main window back where it was if that still fits a
/// monitor that is connected now; otherwise it keeps its default size and position (still
/// maximized if it was). `minimum`: the window's minimum inner size in logical pixels.
pub fn restore<R: Runtime>(window: &WebviewWindow<R>, placement: &Placement, minimum: (f64, f64)) {
    let saved = placement.current();
    let monitors = window.available_monitors().unwrap_or_default();
    let fitting = saved.and_then(|saved| fit(saved, &monitors, minimum));
    let maximized = saved.is_some_and(|saved| saved.maximized);
    match fitting {
        Some(fitting) => {
            let position = PhysicalPosition::new(fitting.x, fitting.y);
            // Position first: on a monitor with other scaling Windows resizes the window itself.
            let _ = window.set_position(position);
            let _ = window.set_size(PhysicalSize::new(fitting.width, fitting.height));
            let _ = window.set_position(position);
            placement.update(WindowPlacement { maximized: false, ..fitting });
        }
        None => {
            // Default bounds, so that a maximized window has bounds to return to.
            if let (Ok(position), Ok(size), Ok(scale)) =
                (window.outer_position(), window.inner_size(), window.scale_factor())
            {
                placement.update(WindowPlacement {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                    scale,
                    maximized: false,
                });
            }
        }
    }
    if maximized {
        let _ = window.maximize();
    }
}

/// `saved`, grown to the minimum size, if it lies on a monitor that is connected now with the
/// same scaling: no larger than its work area, at least half on it, the title bar on it.
fn fit(
    saved: WindowPlacement,
    monitors: &[Monitor],
    minimum: (f64, f64),
) -> Option<WindowPlacement> {
    let scale = saved.scale;
    let width = saved.width.max((minimum.0 * scale).ceil() as u32);
    let height = saved.height.max((minimum.1 * scale).ceil() as u32);
    let (left, top) = (i64::from(saved.x), i64::from(saved.y));
    let (right, bottom) = (left + i64::from(width), top + i64::from(height));
    let px = |logical: f64| (logical * scale).round() as i64;
    monitors.iter().filter(|monitor| (monitor.scale_factor() - scale).abs() < 0.01).find_map(
        |monitor| {
            let area = monitor.work_area();
            let (l, t) = (i64::from(area.position.x), i64::from(area.position.y));
            let (r, b) = (l + i64::from(area.size.width), t + i64::from(area.size.height));
            // The window frame (and on Windows its invisible resize border) adds a few pixels.
            let slack = px(16.0);
            let fits = i64::from(width) <= r - l + slack && i64::from(height) <= b - t + slack;
            let overlap_x = (right.min(r) - left.max(l)).max(0);
            let overlap_y = (bottom.min(b) - top.max(t)).max(0);
            let half_on_it = 2 * overlap_x * overlap_y >= i64::from(width) * i64::from(height);
            let title_bar_on_it = top >= t - slack && top + px(32.0) <= b && overlap_x >= px(120.0);
            (fits && half_on_it && title_bar_on_it).then_some(WindowPlacement {
                width,
                height,
                ..saved
            })
        },
    )
}
