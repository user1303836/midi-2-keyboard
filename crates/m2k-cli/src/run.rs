//! `m2k run [--config <path>] [--dry-run] [--all]`: runs the real application runtime headless
//! (config + engine + MIDI input, exactly what the desktop app runs) and prints everything that
//! happens. Ctrl+C shuts it down cleanly, which releases everything the engine holds.

use std::collections::HashMap;
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use m2k_core::{
    Config, EngineEvent, EngineStatus, Outcome, Runtime, RuntimeEvent, RuntimeOptions, SkipReason,
    Snapshot, store,
};
use m2k_midi::input::PortStatus;

use crate::devices::{describe_derived, describe_message, describe_state};

/// Runtime events buffered for the terminal before new ones are dropped.
const QUEUE: usize = 4096;

fn usage() -> String {
    format!(
        "usage: m2k run [--config <path>] [--dry-run] [--all]

Runs midi-2-keyboard without its window: loads the config, opens the MIDI inputs it allows and
sends the mapped keys and mouse actions to the OS, printing every MIDI message, fired action,
status and port change. Ctrl+C stops it and releases everything that is held.

  --config <path>  the config file (default: the app's own, {})
  --dry-run        do everything except send input to the OS
  --all            also show clock and active sensing messages",
        store::default_path().display()
    )
}

fn usage_error(message: &str) -> i32 {
    eprintln!("m2k run: {message}\n\n{}", usage());
    2
}

pub fn run(args: &[String]) -> i32 {
    let mut config_path: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut show_all = false;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--config" => match args.next() {
                Some(path) => config_path = Some(PathBuf::from(path)),
                None => return usage_error("--config needs a path"),
            },
            "--dry-run" => dry_run = true,
            "-a" | "--all" => show_all = true,
            "-h" | "--help" => {
                println!("{}", usage());
                return 0;
            }
            other => return usage_error(&format!("unexpected argument {other:?}")),
        }
    }
    let path = config_path.unwrap_or_else(store::default_path);
    let existed = path.exists();

    interrupt::install();

    let (tx, rx) = mpsc::sync_channel::<RuntimeEvent>(QUEUE);
    let dropped = Arc::new(AtomicU64::new(0));
    let sink = {
        let dropped = dropped.clone();
        move |event| {
            if tx.try_send(event).is_err() {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    };

    let started = Instant::now();
    let runtime = Runtime::start_with(&path, RuntimeOptions { dry_run }, sink);
    let snapshot = runtime.snapshot();
    let mut printer = Printer::new(started, show_all, &snapshot.config);
    printer.header(&snapshot, existed, dry_run);

    let mut reported_drops = 0;
    while !interrupt::requested() {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => printer.event(event),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        let drops = dropped.load(Ordering::Relaxed);
        if drops > reported_drops {
            printer.line(
                None,
                "NOTE",
                format!("{} events dropped: the terminal cannot keep up", drops - reported_drops),
            );
            reported_drops = drops;
        }
    }

    printer.line(None, "STOP", "shutting down");
    runtime.shutdown();
    for event in rx.try_iter() {
        printer.event(event);
    }
    printer.line(None, "STOP", "stopped; everything that was held has been released");
    interrupt::done();
    0
}

struct Printer {
    started: Instant,
    show_all: bool,
    /// Mapping id -> what to call it.
    mappings: HashMap<String, String>,
    /// Profile id -> name.
    profiles: HashMap<String, String>,
    ports: Vec<PortStatus>,
    last_status: Option<String>,
}

impl Printer {
    fn new(started: Instant, show_all: bool, config: &Config) -> Self {
        let mut printer = Printer {
            started,
            show_all,
            mappings: HashMap::new(),
            profiles: HashMap::new(),
            ports: Vec::new(),
            last_status: None,
        };
        printer.index(config);
        printer
    }

    fn index(&mut self, config: &Config) {
        self.mappings.clear();
        self.profiles.clear();
        for profile in &config.profiles {
            self.profiles.insert(profile.id.clone(), profile.name.clone());
            for mapping in &profile.mappings {
                let label = if mapping.name.trim().is_empty() {
                    format!("mapping {}", mapping.id)
                } else {
                    format!("{:?}", mapping.name)
                };
                self.mappings.insert(mapping.id.clone(), label);
            }
        }
    }

    fn mapping(&self, id: &str) -> String {
        if id.is_empty() {
            return "(test action)".into();
        }
        self.mappings.get(id).cloned().unwrap_or_else(|| format!("mapping {id}"))
    }

    fn profile(&self, id: Option<&String>) -> String {
        match id {
            Some(id) => format!("{:?}", self.profiles.get(id).unwrap_or(id)),
            None => "none".into(),
        }
    }

    /// One output line: seconds, tag, text. Engine events carry their own timestamp.
    fn line(&self, time_us: Option<u64>, tag: &str, text: impl Display) {
        let us = time_us.unwrap_or_else(|| self.started.elapsed().as_micros() as u64);
        println!("{:>10.3}  {tag:<5}  {text}", us as f64 / 1_000_000.0);
    }

    fn header(&self, snapshot: &Snapshot, existed: bool, dry_run: bool) {
        let config = &snapshot.config;
        let mappings: usize = config.profiles.iter().map(|p| p.mappings.len()).sum();
        let created = if existed { "" } else { " (did not exist: created with the defaults)" };
        println!("m2k run {}", snapshot.info.version);
        println!("  config   {}{created}", snapshot.info.config_path);
        println!(
            "           {} profile(s), {mappings} mapping(s), {} device(s) known",
            config.profiles.len(),
            config.devices.len()
        );
        if dry_run {
            println!("  output   DRY RUN: nothing is sent to the OS");
        } else if snapshot.backend_error.is_empty() {
            println!("  output   {}", snapshot.engine.backend);
        } else {
            println!("  output   NOT WORKING: {}", snapshot.backend_error);
        }
        if !snapshot.permissions.hint.is_empty() {
            println!("  note     {}", snapshot.permissions.hint);
        }
        for warning in &snapshot.warnings {
            println!("  warning  {warning}");
        }
        println!("Press Ctrl+C to stop.");
    }

    fn event(&mut self, event: RuntimeEvent) {
        match event {
            RuntimeEvent::Engine { events } => {
                for event in events {
                    self.engine_event(event);
                }
            }
            RuntimeEvent::Ports { ports } => self.ports_changed(ports),
            RuntimeEvent::Permissions { permissions, backend_error } => {
                let output = if permissions.can_inject { "works" } else { "blocked" };
                let foreground =
                    if permissions.can_detect_foreground { "detected" } else { "not detectable" };
                let mut text = format!("output {output}, foreground app {foreground}");
                if !permissions.hint.is_empty() {
                    text.push_str(&format!(": {}", permissions.hint));
                }
                if !backend_error.is_empty() {
                    text.push_str(&format!("; backend error: {backend_error}"));
                }
                self.line(None, "PERM", text);
            }
            RuntimeEvent::Config { config } => {
                self.index(&config);
                let mappings: usize = config.profiles.iter().map(|p| p.mappings.len()).sum();
                self.line(
                    None,
                    "CONF",
                    format!(
                        "config changed: {} profile(s), {mappings} mapping(s)",
                        config.profiles.len()
                    ),
                );
            }
        }
    }

    fn engine_event(&mut self, event: EngineEvent) {
        match event {
            EngineEvent::Midi(log) => {
                if log.message.is_noise() && !self.show_all {
                    return;
                }
                let mut text = format!("{}  {}", log.device, describe_message(&log.message));
                for derived in &log.derived {
                    text.push_str(&format!("  [{}]", describe_derived(derived)));
                }
                if !log.mappings.is_empty() {
                    let names: Vec<String> =
                        log.mappings.iter().map(|id| self.mapping(id)).collect();
                    text.push_str(&format!("  -> {}", names.join(", ")));
                }
                self.line(Some(log.time_us), "MIDI", text);
            }
            EngineEvent::Fired(fired) => {
                let outcome = match &fired.outcome {
                    Outcome::Sent => "sent".to_owned(),
                    Outcome::Skipped { reason } => format!("skipped: {}", skip_reason(*reason)),
                    Outcome::Failed { error } => format!("FAILED: {error}"),
                };
                let text = format!(
                    "{}: {}  [{outcome}]",
                    self.mapping(&fired.mapping_id),
                    fired.description
                );
                self.line(Some(fired.time_us), "FIRE", text);
            }
            EngineEvent::Status(status) => {
                let text = self.describe_status(&status);
                if self.last_status.as_ref() != Some(&text) {
                    self.line(None, "STAT", &text);
                    self.last_status = Some(text);
                }
            }
            EngineEvent::Error { message } => self.line(None, "ERROR", message),
        }
    }

    fn describe_status(&self, status: &EngineStatus) -> String {
        let mut text = format!("profile {}", self.profile(status.effective_profile.as_ref()));
        if status.auto_switched {
            text.push_str(" (auto)");
        }
        if status.selected_profile != status.effective_profile {
            text.push_str(&format!(
                " [selected {}]",
                self.profile(status.selected_profile.as_ref())
            ));
        }
        if !status.layers.is_empty() {
            text.push_str(&format!("  layers [{}]", status.layers.join(", ")));
        }
        let held: Vec<String> = status.held_keys.iter().map(|key| key.label()).collect();
        text.push_str(&format!("  held [{}]", held.join(" ")));
        if !status.engaged_mappings.is_empty() {
            let engaged: Vec<String> =
                status.engaged_mappings.iter().map(|id| self.mapping(id)).collect();
            text.push_str(&format!("  engaged [{}]", engaged.join(", ")));
        }
        for (flag, name) in [
            (status.paused, "PAUSED"),
            (status.focus_suspended, "SUSPENDED (own window focused)"),
            (status.learning, "LEARNING"),
        ] {
            if flag {
                text.push_str("  ");
                text.push_str(name);
            }
        }
        if let Some(app) = &status.foreground {
            text.push_str(&format!("  app {:?}", app.process));
        }
        text
    }

    fn ports_changed(&mut self, ports: Vec<PortStatus>) {
        for port in &ports {
            match self.ports.iter().find(|p| p.name == port.name) {
                None => {
                    self.line(None, "PORT", format!("+ {}: {}", port.name, describe_state(port)))
                }
                Some(old) if old.state != port.state || old.error != port.error => {
                    self.line(None, "PORT", format!("~ {}: {}", port.name, describe_state(port)));
                }
                Some(_) => {}
            }
        }
        for old in &self.ports {
            if !ports.iter().any(|p| p.name == old.name) {
                self.line(None, "PORT", format!("- {}: removed", old.name));
            }
        }
        self.ports = ports;
    }
}

fn skip_reason(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::Paused => "paused",
        SkipReason::WindowFocused => "own window focused",
        SkipReason::Learning => "learning",
        SkipReason::RateLimited => "rate limited",
        SkipReason::ConditionFailed => "condition not met",
    }
}

/// Ctrl+C (and on Unix SIGTERM) asks the main loop to shut down cleanly; a second Ctrl+C exits
/// right away. Hand-written so the CLI needs no extra dependency: the handlers only flip a flag.
mod interrupt {
    use std::sync::atomic::{AtomicBool, Ordering};

    static REQUESTED: AtomicBool = AtomicBool::new(false);
    static DONE: AtomicBool = AtomicBool::new(false);

    pub fn requested() -> bool {
        REQUESTED.load(Ordering::SeqCst)
    }

    /// The clean shutdown has finished.
    pub fn done() {
        DONE.store(true, Ordering::SeqCst);
    }

    #[cfg(windows)]
    pub fn install() {
        type Handler = unsafe extern "system" fn(u32) -> i32;
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn SetConsoleCtrlHandler(handler: Option<Handler>, add: i32) -> i32;
        }
        const CTRL_BREAK_EVENT: u32 = 1;

        /// Runs on a thread the system creates for each console event.
        unsafe extern "system" fn on_event(kind: u32) -> i32 {
            let again = REQUESTED.swap(true, Ordering::SeqCst);
            if kind <= CTRL_BREAK_EVENT {
                if again {
                    std::process::exit(130);
                }
            } else {
                // Console closed, logoff or shutdown: the process ends as soon as this returns,
                // so give the clean shutdown a moment (Windows allows about 5 seconds).
                for _ in 0..40 {
                    if DONE.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
            }
            1
        }

        if unsafe { SetConsoleCtrlHandler(Some(on_event), 1) } == 0 {
            eprintln!(
                "note: could not install the Ctrl+C handler; Ctrl+C will not release held keys"
            );
        }
    }

    #[cfg(unix)]
    pub fn install() {
        use std::ffi::c_int;
        unsafe extern "C" {
            fn signal(signum: c_int, handler: extern "C" fn(c_int)) -> usize;
            fn _exit(status: c_int) -> !;
        }
        const SIGINT: c_int = 2;
        const SIGTERM: c_int = 15;

        extern "C" fn on_signal(_: c_int) {
            if REQUESTED.swap(true, Ordering::SeqCst) {
                unsafe { _exit(130) };
            }
        }

        unsafe {
            signal(SIGINT, on_signal);
            signal(SIGTERM, on_signal);
        }
    }

    #[cfg(not(any(windows, unix)))]
    pub fn install() {}
}
