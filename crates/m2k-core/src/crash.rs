//! The crash log: what was going on when the app died, and telling the user once.
//!
//! A panic outside the app's own threads (on the main thread, in an OS callback, in a library)
//! usually ends the process. [`install`] adds a panic hook that appends an entry to `crash.log`
//! in the config directory, after the hooks installed before it have run (install it after
//! `m2k_output::install_panic_release`, so every held key is released first). Panics on the
//! app's own `m2k-` threads are logged too, marked `internal` instead of `crash`: the engine
//! recovers from those by itself, so they only help whoever debugs it later. The file keeps the
//! last few entries. The hook uses plain `std::fs` and no lock, and never runs inside itself.
//!
//! At the next start [`startup_notice`] says whether the previous run crashed (a `crash` entry
//! newer than the last clean exit, which [`clean_exit`] records) and that has not been reported
//! yet ([`mark_reported`]).

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The log, in the config directory.
pub const LOG_FILE: &str = "crash.log";
/// When the app last exited cleanly, and which crash was last reported.
const STATE_FILE: &str = "session.json";
/// Entries kept in the log.
const KEEP: usize = 5;
/// A log larger than this is not read back (it is not one this module wrote).
const MAX_LOG_BYTES: u64 = 1 << 20;
const HEADER: &str = "=== ";

#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionState {
    #[serde(default)]
    clean_exit_ms: u64,
    #[serde(default)]
    crash_reported_ms: u64,
}

/// A crash of the previous run, not reported yet.
#[derive(Debug, Clone)]
pub struct Notice {
    /// For the user.
    pub message: String,
    /// The crash entry's time, see [`mark_reported`].
    at_ms: u64,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Adds the crash-log hook on top of the current panic hook, which runs first. `app` names the
/// program and its version in each entry. Installing it twice does nothing.
pub fn install(dir: PathBuf, app: &'static str) {
    static INSTALLED: AtomicBool = AtomicBool::new(false);
    if INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Release what is held and report the panic first.
        previous(info);
        // One entry at a time, and never for a panic inside the writing itself.
        static WRITING: AtomicBool = AtomicBool::new(false);
        if WRITING.swap(true, Ordering::SeqCst) {
            return;
        }
        write_entry(&dir, app, info);
        WRITING.store(false, Ordering::SeqCst);
    }));
}

fn write_entry(dir: &Path, app: &str, info: &std::panic::PanicHookInfo<'_>) {
    let thread = std::thread::current();
    let name = thread.name().unwrap_or("unnamed");
    let kind = if name.starts_with("m2k-") { "internal" } else { "crash" };
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("(no message)");
    let location = info
        .location()
        .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
        .unwrap_or_else(|| "an unknown place".into());
    // The release builds are stripped: the frames are addresses, still useful against the
    // unstripped build of the same version.
    let backtrace = std::backtrace::Backtrace::force_capture();
    let at = now_ms();
    let entry = format!(
        "{HEADER}{at} {} | {kind} | {app} | thread {name}\npanicked at {location}:\n{message}\n\
         backtrace:\n{backtrace}\n",
        utc(at)
    );
    let path = dir.join(LOG_FILE);
    let mut entries = read_log(&path).map(|text| split(&text)).unwrap_or_default();
    entries.push(entry);
    let first = entries.len().saturating_sub(KEEP);
    let text: String = entries.iter().skip(first).map(String::as_str).collect();
    let _ = fs::create_dir_all(dir);
    let _ = fs::write(&path, text);
}

fn read_log(path: &Path) -> Option<String> {
    let size = fs::metadata(path).ok()?.len();
    if size > MAX_LOG_BYTES {
        return None;
    }
    fs::read_to_string(path).ok()
}

/// The entries of a log, each with its header line.
fn split(text: &str) -> Vec<String> {
    let mut entries: Vec<String> = Vec::new();
    for line in text.split_inclusive('\n') {
        if line.starts_with(HEADER) || entries.is_empty() {
            entries.push(String::new());
        }
        if let Some(entry) = entries.last_mut() {
            entry.push_str(line);
        }
    }
    entries
}

/// Time and kind from an entry's header line (`=== <ms> <date> <time> UTC | <kind> | ...`).
fn header(entry: &str) -> Option<(u64, &str)> {
    let line = entry.strip_prefix(HEADER)?.lines().next()?;
    let at = line.split(' ').next()?.parse().ok()?;
    let kind = line.split(" | ").nth(1)?;
    Some((at, kind))
}

/// What to tell the user about the previous run: `Some` once after it crashed.
pub fn startup_notice(dir: &Path) -> Option<Notice> {
    let path = dir.join(LOG_FILE);
    let text = read_log(&path)?;
    let entries = split(&text);
    let newest = entries
        .iter()
        .filter_map(|entry| header(entry))
        .filter(|(_, kind)| *kind == "crash")
        .map(|(at, _)| at)
        .max()?;
    let state = read_state(dir);
    (newest > state.clean_exit_ms && newest > state.crash_reported_ms).then(|| Notice {
        message: format!(
            "MIDI 2 Keyboard closed unexpectedly last time (an internal error). Everything it was \
             holding was released. Details: {}",
            path.display()
        ),
        at_ms: newest,
    })
}

/// `notice` has been shown: it is not shown again.
pub fn mark_reported(dir: &Path, notice: &Notice) {
    let mut state = read_state(dir);
    state.crash_reported_ms = state.crash_reported_ms.max(notice.at_ms);
    write_state(dir, &state);
}

/// The app is exiting normally (crashes before this one are no longer news).
pub fn clean_exit(dir: &Path) {
    let mut state = read_state(dir);
    state.clean_exit_ms = now_ms();
    write_state(dir, &state);
}

fn read_state(dir: &Path) -> SessionState {
    fs::read_to_string(dir.join(STATE_FILE))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_state(dir: &Path, state: &SessionState) {
    if let Ok(text) = serde_json::to_string_pretty(state) {
        let _ = fs::create_dir_all(dir);
        let _ = fs::write(dir.join(STATE_FILE), text + "\n");
    }
}

/// `2026-11-27 18:42:03 UTC` for milliseconds since the Unix epoch.
fn utc(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // Days to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}
