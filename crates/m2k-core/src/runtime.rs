//! The application runtime: owns the config, the engine and the MIDI input manager, and wires
//! them together. Front ends (the desktop shell, the CLI) talk only to this.
//!
//! Threads: MIDI callbacks call straight into [`Engine::midi`] (which only posts a message).
//! Engine events are collected and handed to the front end in batches by a small flusher thread,
//! so a busy MIDI stream costs one IPC message per frame, not one per MIDI message.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::thread;
use std::time::Duration;

use m2k_midi::input::{InputEvent, InputManager, InputOptions, PortPolicy, PortState, PortStatus};
use m2k_output::platform::{self, Permissions};
use m2k_output::{Injector, NullInjector};

use crate::app::{AppInfo, RuntimeEvent, Snapshot};
use crate::engine::{Engine, EngineOptions};
use crate::events::{EngineEvent, EngineStatus};
use crate::model::{Action, Config};
use crate::store;

/// How long engine events are gathered before being pushed to the front end.
const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
/// Upper bound on buffered monitor events (a hidden or stalled front end must not grow memory).
const MAX_PENDING: usize = 4096;
/// How often permissions / the output backend are re-checked while something is wrong.
const PERMISSION_POLL: Duration = Duration::from_secs(2);

type Sink = dyn Fn(RuntimeEvent) + Send + Sync;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Default)]
struct Pending {
    events: Vec<EngineEvent>,
    /// The latest status, delivered even when monitor events are being dropped.
    status: Option<EngineStatus>,
}

struct Inner {
    path: PathBuf,
    /// Never build a real injector (see [`RuntimeOptions::dry_run`]).
    dry_run: bool,
    sink: Box<Sink>,
    config: Mutex<Config>,
    engine: Engine,
    input: Mutex<Option<InputManager>>,
    ports: Mutex<Vec<PortStatus>>,
    permissions: Mutex<Permissions>,
    backend_error: Mutex<String>,
    warnings: Mutex<Vec<String>>,
    pending: Mutex<Pending>,
    wake: Condvar,
    /// Forward monitor events (MIDI / fired). Off while no window is showing them.
    monitoring: AtomicBool,
    stopping: AtomicBool,
}

/// Handle to the running application core. Cheap to clone.
#[derive(Clone)]
pub struct Runtime {
    inner: Arc<Inner>,
}

/// How [`Runtime::start_with`] runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimeOptions {
    /// Run everything (MIDI input, matching, behaviors, monitor events) but never send input to
    /// the OS: the engine always gets a [`NullInjector`], also after injector settings change.
    pub dry_run: bool,
}

fn make_injector(config: &Config, dry_run: bool) -> (Box<dyn Injector>, String) {
    if dry_run {
        return (Box::new(NullInjector), String::new());
    }
    match m2k_output::create_injector(&config.settings.injector) {
        Ok(injector) => (injector, String::new()),
        Err(error) => (Box::new(NullInjector), error.to_string()),
    }
}

fn policy_for(config: &Config) -> PortPolicy {
    if config.settings.connect_new_devices {
        PortPolicy::AllExcept(
            config.devices.iter().filter(|d| !d.enabled).map(|d| d.name.clone()).collect(),
        )
    } else {
        PortPolicy::Only(
            config.devices.iter().filter(|d| d.enabled).map(|d| d.name.clone()).collect(),
        )
    }
}

fn platform_name() -> &'static str {
    match std::env::consts::OS {
        "windows" => "windows",
        "macos" => "macos",
        _ => "linux",
    }
}

fn is_wayland() -> bool {
    cfg!(target_os = "linux")
        && (std::env::var_os("WAYLAND_DISPLAY").is_some()
            || std::env::var("XDG_SESSION_TYPE").is_ok_and(|v| v.eq_ignore_ascii_case("wayland")))
}

impl Runtime {
    /// Loads the config at `path` and starts everything. `sink` receives live updates from
    /// background threads.
    pub fn start(path: &Path, sink: impl Fn(RuntimeEvent) + Send + Sync + 'static) -> Runtime {
        Self::start_with(path, RuntimeOptions::default(), sink)
    }

    /// [`start`](Self::start) with options (e.g. a dry run that never sends input to the OS).
    pub fn start_with(
        path: &Path,
        options: RuntimeOptions,
        sink: impl Fn(RuntimeEvent) + Send + Sync + 'static,
    ) -> Runtime {
        let loaded = store::load(path);
        let config = loaded.config;
        if loaded.created {
            // Materialize the default config so the user has a file to look at / back up.
            if let Err(error) = store::save(path, &config) {
                log::warn!("could not write the initial config: {error}");
            }
        }
        let (injector, backend_error) = make_injector(&config, options.dry_run);

        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| {
            let for_engine = weak.clone();
            let engine = Engine::start(
                config.clone(),
                EngineOptions { injector, track_foreground: true },
                move |event| {
                    if let Some(inner) = for_engine.upgrade() {
                        inner.on_engine_event(event);
                    }
                },
            );
            Inner {
                path: path.to_path_buf(),
                dry_run: options.dry_run,
                sink: Box::new(sink),
                config: Mutex::new(config.clone()),
                engine,
                input: Mutex::new(None),
                ports: Mutex::new(Vec::new()),
                permissions: Mutex::new(platform::permissions()),
                backend_error: Mutex::new(backend_error),
                warnings: Mutex::new(loaded.warnings),
                pending: Mutex::new(Pending::default()),
                wake: Condvar::new(),
                monitoring: AtomicBool::new(true),
                stopping: AtomicBool::new(false),
            }
        });

        // MIDI input.
        let for_input = Arc::downgrade(&inner);
        let manager = InputManager::start(
            InputOptions {
                virtual_port: cfg!(unix).then(|| "MIDI 2 Keyboard".to_string()),
                ..InputOptions::default()
            },
            policy_for(&config),
            move |event| {
                if let Some(inner) = for_input.upgrade() {
                    inner.on_input_event(event);
                }
            },
        );
        *lock(&inner.ports) = manager.ports();
        *lock(&inner.input) = Some(manager);

        // Event flusher.
        let for_flusher = Arc::downgrade(&inner);
        let _ = thread::Builder::new().name("m2k-events".into()).spawn(move || {
            while let Some(inner) = for_flusher.upgrade() {
                if !inner.flush_once() {
                    break;
                }
            }
        });

        // Permission / backend watcher.
        let for_watcher = Arc::downgrade(&inner);
        let _ = thread::Builder::new().name("m2k-permissions".into()).spawn(move || {
            loop {
                thread::sleep(PERMISSION_POLL);
                let Some(inner) = for_watcher.upgrade() else { break };
                if inner.stopping.load(Ordering::Relaxed) {
                    break;
                }
                inner.check_permissions();
            }
        });

        Runtime { inner }
    }

    /// Everything a front end needs for its first render. Consumes the load warnings.
    pub fn snapshot(&self) -> Snapshot {
        let inner = &self.inner;
        Snapshot {
            info: AppInfo {
                version: env!("CARGO_PKG_VERSION").to_string(),
                platform: platform_name().to_string(),
                config_path: inner.path.display().to_string(),
                wayland: is_wayland(),
            },
            config: lock(&inner.config).clone(),
            engine: inner.engine.status(),
            ports: lock(&inner.ports).clone(),
            permissions: lock(&inner.permissions).clone(),
            warnings: std::mem::take(&mut *lock(&inner.warnings)),
            backend_error: lock(&inner.backend_error).clone(),
        }
    }

    pub fn config(&self) -> Config {
        lock(&self.inner.config).clone()
    }

    pub fn status(&self) -> EngineStatus {
        self.inner.engine.status()
    }

    /// Validates, applies and persists a new config. Returns the config as actually applied.
    pub fn set_config(&self, mut config: Config) -> Result<Config, store::StoreError> {
        let inner = &self.inner;
        store::sanitize(&mut config);
        let previous = std::mem::replace(&mut *lock(&inner.config), config.clone());
        if previous == config {
            return Ok(config);
        }
        if previous.settings.injector != config.settings.injector {
            inner.rebuild_injector(&config);
        }
        inner.engine.set_config(config.clone());
        if let Some(input) = lock(&inner.input).as_ref() {
            let policy = policy_for(&config);
            if policy != policy_for(&previous) {
                input.set_policy(policy);
            }
        }
        store::save(&inner.path, &config)?;
        Ok(config)
    }

    pub fn set_paused(&self, paused: bool) {
        self.inner.engine.set_paused(paused);
    }

    pub fn set_learning(&self, learning: bool) {
        self.inner.engine.set_learning(learning);
    }

    /// The front end's own window gained / lost keyboard focus.
    pub fn set_window_focused(&self, focused: bool) {
        self.inner.engine.set_window_focused(focused);
    }

    /// Whether anybody is looking at the monitor. While off, per-message events are not
    /// collected at all (status updates still are).
    pub fn set_monitoring(&self, enabled: bool) {
        self.inner.monitoring.store(enabled, Ordering::Relaxed);
        if !enabled {
            lock(&self.inner.pending).events.clear();
        }
    }

    pub fn select_profile(&self, profile_id: &str) {
        self.inner.engine.select_profile(profile_id);
    }

    /// Feeds MIDI bytes as if `device` had sent them.
    pub fn send_midi(&self, device: &str, bytes: &[u8]) {
        if let Some(input) = lock(&self.inner.input).as_ref() {
            input.inject(device, bytes);
        }
    }

    pub fn test_action(&self, action: Action, delay_ms: u32) {
        self.inner.engine.run_action(action, delay_ms);
    }

    pub fn release_all(&self) {
        self.inner.engine.release_all();
    }

    pub fn rescan_devices(&self) {
        if let Some(input) = lock(&self.inner.input).as_ref() {
            input.rescan();
        }
    }

    pub fn request_permissions(&self) {
        platform::request_permissions();
        self.inner.check_permissions();
    }

    pub fn config_path(&self) -> PathBuf {
        self.inner.path.clone()
    }

    /// Stops MIDI input and releases everything the engine holds. Call before exiting.
    pub fn shutdown(&self) {
        let inner = &self.inner;
        if inner.stopping.swap(true, Ordering::SeqCst) {
            return;
        }
        drop(lock(&inner.input).take());
        inner.engine.stop();
        inner.wake.notify_all();
    }
}

impl Inner {
    fn emit(&self, event: RuntimeEvent) {
        (self.sink)(event);
    }

    fn on_engine_event(&self, event: EngineEvent) {
        match event {
            EngineEvent::Status(status) => {
                self.follow_profile_selection(&status);
                lock(&self.pending).status = Some(status);
            }
            other => {
                let monitor = matches!(other, EngineEvent::Midi(_) | EngineEvent::Fired(_));
                if monitor && !self.monitoring.load(Ordering::Relaxed) {
                    return;
                }
                let mut pending = lock(&self.pending);
                if pending.events.len() < MAX_PENDING {
                    pending.events.push(other);
                }
            }
        }
        self.wake.notify_one();
    }

    /// An action (or the tray) changed the selected profile: remember it across restarts.
    fn follow_profile_selection(&self, status: &EngineStatus) {
        let Some(selected) = &status.selected_profile else { return };
        let mut config = lock(&self.config);
        if config.selected_profile.as_ref() == Some(selected)
            || !config.profiles.iter().any(|p| &p.id == selected)
        {
            return;
        }
        config.selected_profile = Some(selected.clone());
        let snapshot = config.clone();
        drop(config);
        if let Err(error) = store::save(&self.path, &snapshot) {
            log::warn!("could not persist the selected profile: {error}");
        }
    }

    /// Waits for events and pushes one batch. Returns `false` when the runtime is stopping.
    fn flush_once(&self) -> bool {
        {
            let mut pending = lock(&self.pending);
            while pending.events.is_empty() && pending.status.is_none() {
                if self.stopping.load(Ordering::Relaxed) {
                    return false;
                }
                let (guard, _) = self
                    .wake
                    .wait_timeout(pending, Duration::from_millis(500))
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                pending = guard;
            }
        }
        // Let a burst accumulate so it travels as one batch.
        thread::sleep(FLUSH_INTERVAL);
        let (mut events, status) = {
            let mut pending = lock(&self.pending);
            (std::mem::take(&mut pending.events), pending.status.take())
        };
        if let Some(status) = status {
            events.push(EngineEvent::Status(status));
        }
        if !events.is_empty() {
            self.emit(RuntimeEvent::Engine { events });
        }
        !self.stopping.load(Ordering::Relaxed)
    }

    fn on_input_event(&self, event: InputEvent) {
        match event {
            InputEvent::Message { port, time_us, message } => {
                self.engine.midi(&port, time_us, message);
            }
            InputEvent::PortsChanged { ports } => {
                let previous = std::mem::replace(&mut *lock(&self.ports), ports.clone());
                for old in previous.iter().filter(|p| p.state == PortState::Connected) {
                    let still_connected = ports
                        .iter()
                        .any(|p| p.name == old.name && p.state == PortState::Connected);
                    if !still_connected {
                        self.engine.device_disconnected(&old.name);
                    }
                }
                self.emit(RuntimeEvent::Ports { ports });
            }
        }
    }

    fn rebuild_injector(&self, config: &Config) {
        let (injector, error) = make_injector(config, self.dry_run);
        self.engine.set_injector(injector);
        *lock(&self.backend_error) = error;
        self.publish_permissions();
    }

    fn publish_permissions(&self) {
        self.emit(RuntimeEvent::Permissions {
            permissions: lock(&self.permissions).clone(),
            backend_error: lock(&self.backend_error).clone(),
        });
    }

    /// Re-reads OS permissions; when something changed (the user granted access, a session
    /// came up) retries the output backend.
    fn check_permissions(&self) {
        let current = platform::permissions();
        let changed = {
            let mut stored = lock(&self.permissions);
            let changed = *stored != current;
            *stored = current;
            changed
        };
        let broken = !lock(&self.backend_error).is_empty();
        if broken {
            // No working backend yet: try again (the user may just have granted access).
            let config = lock(&self.config).clone();
            let (injector, error) = make_injector(&config, self.dry_run);
            if error.is_empty() {
                self.engine.set_injector(injector);
            }
            let error_changed = *lock(&self.backend_error) != error;
            *lock(&self.backend_error) = error;
            if error_changed || changed {
                self.publish_permissions();
            }
        } else if changed {
            self.publish_permissions();
        }
    }
}
