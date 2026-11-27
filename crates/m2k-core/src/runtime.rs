//! The application runtime: owns the config, the engine and the MIDI input manager, and wires
//! them together. Front ends (the desktop shell, the CLI) talk only to this.
//!
//! Threads: MIDI callbacks call straight into [`Engine::midi`] (which only posts a message).
//! Engine events are collected and handed to the front end in batches by a small flusher thread,
//! so a busy MIDI stream costs one IPC message per frame, not one per MIDI message.
//!
//! The config file may also be edited by hand: a watcher thread notices changes made by
//! anything else, applies a valid edit like [`Runtime::set_config`] (without rewriting the
//! file) and reports an invalid one while the previous configuration stays active.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, Weak};
use std::thread;
use std::time::{Duration, SystemTime};

use m2k_midi::input::{InputEvent, InputManager, InputOptions, PortPolicy, PortState, PortStatus};
use m2k_midi::output::{OutputEvent, OutputManager, OutputOptions, SendError, paired_output_name};
use m2k_output::platform::{self, Permissions};
use m2k_output::{Injector, Key, NullInjector};

use crate::app::{AppInfo, RuntimeEvent, Snapshot};
use crate::engine::{Engine, EngineOptions, MidiSink};
use crate::events::{EngineEvent, EngineStatus};
use crate::model::{Action, Config};
use crate::store;

/// How long engine events are gathered before being pushed to the front end.
const FLUSH_INTERVAL: Duration = Duration::from_millis(16);
/// Upper bound on buffered monitor events (a hidden or stalled front end must not grow memory).
const MAX_PENDING: usize = 4096;
/// How often permissions / the output backend are re-checked while something is wrong.
const PERMISSION_POLL: Duration = Duration::from_secs(2);
/// How often the config file is checked for changes made by someone else. A change must hold
/// still for one more check before it is read (editors write via temp file + rename, or
/// truncate and write).
const CONFIG_POLL: Duration = Duration::from_secs(1);

type Sink = dyn Fn(RuntimeEvent) + Send + Sync;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// What the config file looked like when we last read or wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FileStamp {
    modified: Option<SystemTime>,
    len: u64,
}

impl FileStamp {
    /// `None` when the file does not exist (or cannot be inspected).
    fn of(path: &Path) -> Option<FileStamp> {
        let meta = fs::metadata(path).ok()?;
        Some(FileStamp { modified: meta.modified().ok(), len: meta.len() })
    }
}

#[derive(Default)]
struct ConfigFile {
    stamp: Option<FileStamp>,
    /// The file's content as we last read or wrote it.
    bytes: Vec<u8>,
    /// A changed stamp seen by the previous check; read once it holds still.
    settling: Option<Option<FileStamp>>,
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
    /// MIDI output ("Send MIDI", feedback lights). `None` in a dry run.
    output: Mutex<Option<Arc<OutputManager>>>,
    outputs: Mutex<Vec<PortStatus>>,
    /// The output port names, shared with the engine's [`OutputSink`].
    output_names: Arc<Mutex<Vec<String>>>,
    /// Outputs switched off under MIDI devices (and the outputs paired with those inputs):
    /// the manager's policy, shared with the engine's [`OutputSink`].
    output_disabled: Arc<Mutex<Vec<String>>>,
    permissions: Mutex<Permissions>,
    backend_error: Mutex<String>,
    /// Keys the current output backend cannot send (shown as unavailable in the UI).
    unsupported_keys: Mutex<Vec<Key>>,
    warnings: Mutex<Vec<String>>,
    pending: Mutex<Pending>,
    /// The config file as we know it. Held while saving and while checking for outside
    /// changes, so the two never interleave.
    file: Mutex<ConfigFile>,
    /// Serializes applying configs (a hand edit and a UI change must not interleave).
    applying: Mutex<()>,
    /// The running config must be written to disk (set off the engine thread, written by the
    /// config watcher thread, which `save_wake` wakes).
    save_pending: Mutex<bool>,
    save_wake: Condvar,
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

/// The keys `injector` cannot send.
fn unsupported_keys(injector: &dyn Injector) -> Vec<Key> {
    Key::ALL.iter().copied().filter(|key| !injector.supports(*key)).collect()
}

/// The app's own virtual MIDI input (macOS / Linux), which other programs see as an output.
const VIRTUAL_PORT: &str = "MIDI 2 Keyboard";

/// Set (to anything non-empty): no MIDI is sent at all, the outputs are only listed. For agents
/// and cautious users who run the real app.
const NO_MIDI_OUT: &str = "M2K_NO_MIDI_OUT";

fn switched_off(port: &str) -> String {
    format!("\u{201C}{port}\u{201D} is switched off under MIDI devices")
}

/// The engine's MIDI output: m2k-midi's [`OutputManager`], with the port names and the switched
/// off ones at hand so the engine thread never copies the port list.
struct OutputSink {
    manager: Arc<OutputManager>,
    names: Arc<Mutex<Vec<String>>>,
    disabled: Arc<Mutex<Vec<String>>>,
}

impl MidiSink for OutputSink {
    fn send(&self, port: &str, bytes: &[u8]) -> Result<(), String> {
        self.manager.send(Some(port), bytes).map_err(|error| match error {
            SendError::Disabled(port) => switched_off(&port),
            other => other.to_string(),
        })
    }

    fn has_port(&self, name: &str) -> bool {
        lock(&self.names).iter().any(|port| port == name)
    }

    fn unavailable(&self, port: &str) -> Option<String> {
        if lock(&self.disabled).iter().any(|name| name == port) {
            return Some(switched_off(port));
        }
        (!self.has_port(port)).then(|| format!("there is no MIDI output named \"{port}\""))
    }

    fn paired_output(&self, input: &str) -> Option<String> {
        let names = lock(&self.names);
        paired_output_name(input, names.iter().map(String::as_str)).map(str::to_string)
    }
}

/// No MIDI output at all (a dry run, or `M2K_NO_MIDI_OUT`): every send is refused with `reason`.
struct NoMidiOut {
    reason: &'static str,
}

impl MidiSink for NoMidiOut {
    fn send(&self, _port: &str, _bytes: &[u8]) -> Result<(), String> {
        Err(self.reason.to_string())
    }

    fn has_port(&self, _name: &str) -> bool {
        false
    }

    fn unavailable(&self, _port: &str) -> Option<String> {
        Some(self.reason.to_string())
    }

    fn paired_output(&self, _input: &str) -> Option<String> {
        None
    }
}

/// Outputs to leave alone: the devices switched off under MIDI devices, and the outputs paired
/// with those inputs. Only these: an output is opened only when an action or a light targets it,
/// and output-only ports (a software synth, a sound module) have no input to switch on.
fn disabled_outputs(config: &Config, outputs: &[PortStatus]) -> Vec<String> {
    let mut names = Vec::new();
    for device in config.devices.iter().filter(|device| !device.enabled) {
        names.push(device.name.clone());
        let present = outputs.iter().map(|port| port.name.as_str());
        if let Some(paired) = paired_output_name(&device.name, present) {
            names.push(paired.to_string());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Connected input ports, without the app's own virtual one (where unaimed feedback can go).
fn connected_inputs(ports: &[PortStatus]) -> Vec<String> {
    ports
        .iter()
        .filter(|port| port.state == PortState::Connected && !port.is_virtual)
        .map(|port| port.name.clone())
        .collect()
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
        let (injector, backend_error) = make_injector(&config, options.dry_run);
        let unsupported = unsupported_keys(injector.as_ref());

        // MIDI output, started first so the engine's first lights go to real ports, with the
        // devices switched off under MIDI devices left alone. In a dry run, or with
        // M2K_NO_MIDI_OUT set, the outputs are only listed: nothing is ever sent (a test must not
        // change what the controllers show).
        let no_output = if options.dry_run {
            Some("nothing is sent to MIDI outputs in a dry run")
        } else if std::env::var_os(NO_MIDI_OUT).is_some_and(|value| !value.is_empty()) {
            Some("MIDI output is switched off (M2K_NO_MIDI_OUT)")
        } else {
            None
        };
        let to_inner: Arc<OnceLock<Weak<Inner>>> = Arc::new(OnceLock::new());
        let output = {
            let to_inner = to_inner.clone();
            Arc::new(OutputManager::start(
                OutputOptions {
                    own_virtual_input: cfg!(unix).then(|| VIRTUAL_PORT.to_string()),
                    ..OutputOptions::default()
                },
                PortPolicy::AllExcept(disabled_outputs(&config, &[])),
                move |event| {
                    if let Some(inner) = to_inner.get().and_then(Weak::upgrade) {
                        inner.on_output_event(event);
                    }
                },
            ))
        };
        let outputs = output.ports();
        // With the ports known, the outputs paired with switched-off inputs too (before the
        // engine can send anything).
        let disabled = disabled_outputs(&config, &outputs);
        output.set_policy(PortPolicy::AllExcept(disabled.clone()));
        let output_disabled = Arc::new(Mutex::new(disabled));
        let output_names =
            Arc::new(Mutex::new(outputs.iter().map(|port| port.name.clone()).collect::<Vec<_>>()));
        let midi_out: Box<dyn MidiSink> = match no_output {
            None => Box::new(OutputSink {
                manager: output.clone(),
                names: output_names.clone(),
                disabled: output_disabled.clone(),
            }),
            Some(reason) => Box::new(NoMidiOut { reason }),
        };

        let inner = Arc::new_cyclic(|weak: &Weak<Inner>| {
            let for_engine = weak.clone();
            let engine = Engine::start(
                config.clone(),
                EngineOptions { injector, track_foreground: true, midi_out },
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
                output: Mutex::new(Some(output.clone())),
                outputs: Mutex::new(outputs.clone()),
                output_names,
                output_disabled,
                permissions: Mutex::new(platform::permissions()),
                backend_error: Mutex::new(backend_error),
                unsupported_keys: Mutex::new(unsupported),
                warnings: Mutex::new(loaded.warnings),
                pending: Mutex::new(Pending::default()),
                file: Mutex::new(ConfigFile::default()),
                applying: Mutex::new(()),
                save_pending: Mutex::new(false),
                save_wake: Condvar::new(),
                wake: Condvar::new(),
                monitoring: AtomicBool::new(true),
                stopping: AtomicBool::new(false),
            }
        });

        if loaded.created {
            // Materialize the default config so the user has a file to look at / back up.
            if let Err(error) = inner.save(&config) {
                log::warn!("could not write the initial config: {error}");
            }
        } else {
            inner.remember_file();
        }

        // Output events from now on; a change between the first scan and here still counts.
        let _ = to_inner.set(Arc::downgrade(&inner));
        let now = output.ports();
        if now != outputs {
            inner.on_output_event(OutputEvent::PortsChanged { ports: now });
        }

        // MIDI input.
        let for_input = Arc::downgrade(&inner);
        let manager = InputManager::start(
            InputOptions {
                virtual_port: cfg!(unix).then(|| VIRTUAL_PORT.to_string()),
                ..InputOptions::default()
            },
            policy_for(&config),
            move |event| {
                if let Some(inner) = for_input.upgrade() {
                    inner.on_input_event(event);
                }
            },
        );
        let ports = manager.ports();
        inner.engine.midi_inputs_changed(connected_inputs(&ports));
        *lock(&inner.ports) = ports;
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

        // Config file watcher (hand edits) and writer of saves requested from the engine thread.
        let for_files = Arc::downgrade(&inner);
        let _ = thread::Builder::new().name("m2k-config-watch".into()).spawn(move || {
            loop {
                let Some(inner) = for_files.upgrade() else { break };
                {
                    let pending = lock(&inner.save_pending);
                    if !*pending && !inner.stopping.load(Ordering::Relaxed) {
                        drop(inner.save_wake.wait_timeout(pending, CONFIG_POLL));
                    }
                }
                inner.flush_pending_save();
                if inner.stopping.load(Ordering::Relaxed) {
                    break;
                }
                inner.check_config_file();
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
                elevated: platform::is_elevated(),
            },
            config: lock(&inner.config).clone(),
            engine: inner.engine.status(),
            ports: lock(&inner.ports).clone(),
            outputs: lock(&inner.outputs).clone(),
            permissions: lock(&inner.permissions).clone(),
            warnings: std::mem::take(&mut *lock(&inner.warnings)),
            backend_error: lock(&inner.backend_error).clone(),
            unsupported_keys: lock(&inner.unsupported_keys).clone(),
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
        if !inner.apply(&config) {
            return Ok(config);
        }
        inner.save(&config)?;
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
        self.inner.engine.set_monitoring(enabled);
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

    /// Closes and reopens every MIDI port in use, inputs and outputs, keeping what is switched
    /// off as it is: for when the computer wakes up or the session is unlocked, when a device can
    /// come back under the same name with the old handle dead. The inputs are reported closed
    /// first, so whatever their devices held is released (as for a disconnect); the outputs are
    /// reported afterwards even if nothing seems to have changed, so lights are sent again.
    /// Asynchronous and harmless to call twice.
    pub fn reopen_midi(&self) {
        if let Some(input) = lock(&self.inner.input).as_ref() {
            input.reopen_all();
        }
        if let Some(output) = lock(&self.inner.output).as_ref() {
            output.reopen_all();
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
        // After the engine (whose sink holds it too): delivers what is queued, closes the ports.
        drop(lock(&inner.output).take());
        inner.flush_pending_save();
        inner.wake.notify_all();
        inner.save_wake.notify_all();
    }
}

impl Inner {
    fn emit(&self, event: RuntimeEvent) {
        (self.sink)(event);
    }

    fn emit_error(&self, message: String) {
        self.emit(RuntimeEvent::Engine { events: vec![EngineEvent::Error { message }] });
    }

    /// Makes `config` (already sanitized) the running configuration: engine, output backend
    /// options, MIDI input policy. Does not save. `false` when nothing changed.
    fn apply(&self, config: &Config) -> bool {
        let _applying = lock(&self.applying);
        let previous = std::mem::replace(&mut *lock(&self.config), config.clone());
        if previous == *config {
            return false;
        }
        if previous.settings.injector != config.settings.injector {
            self.rebuild_injector(config);
        }
        // Before the engine sees the config: a device switched off in it must already be
        // refused when the engine sends the lights of the new config.
        self.update_output_policy(config);
        self.engine.set_config(config.clone());
        if let Some(input) = lock(&self.input).as_ref() {
            let policy = policy_for(config);
            if policy != policy_for(&previous) {
                input.set_policy(policy);
            }
        }
        true
    }

    /// Leaves the outputs of switched-off devices alone (the engine asks first; the manager's
    /// policy refuses them as well).
    fn update_output_policy(&self, config: &Config) {
        let outputs = lock(&self.outputs).clone();
        let disabled = disabled_outputs(config, &outputs);
        let mut current = lock(&self.output_disabled);
        if *current == disabled {
            return;
        }
        *current = disabled.clone();
        drop(current);
        if let Some(manager) = lock(&self.output).as_ref() {
            manager.set_policy(PortPolicy::AllExcept(disabled));
        }
    }

    /// Saves the config and remembers what the file looks like now, so the watcher does not
    /// take our own write for a hand edit.
    fn save(&self, config: &Config) -> Result<(), store::StoreError> {
        let mut file = lock(&self.file);
        let result = store::save(&self.path, config);
        file.stamp = FileStamp::of(&self.path);
        file.bytes = fs::read(&self.path).unwrap_or_default();
        file.settling = None;
        result
    }

    /// Writes the running config if the engine thread asked for it (it must never wait for the
    /// disk itself). The config is taken as it is at the moment of writing, under the file
    /// lock, so a newer save can never be overwritten by an older snapshot.
    fn flush_pending_save(&self) {
        if !std::mem::take(&mut *lock(&self.save_pending)) {
            return;
        }
        let mut file = lock(&self.file);
        let config = lock(&self.config).clone();
        if let Err(error) = store::save(&self.path, &config) {
            log::warn!("could not persist the selected profile: {error}");
        }
        file.stamp = FileStamp::of(&self.path);
        file.bytes = fs::read(&self.path).unwrap_or_default();
        file.settling = None;
    }

    /// Records the file as it is now (after loading it).
    fn remember_file(&self) {
        let mut file = lock(&self.file);
        file.stamp = FileStamp::of(&self.path);
        file.bytes = fs::read(&self.path).unwrap_or_default();
        file.settling = None;
    }

    /// One check of the config file for changes made by someone else (a text editor, a sync
    /// tool, a script). A valid change is applied without rewriting the file and announced to
    /// the front end; an invalid one is reported once and the running config stays.
    fn check_config_file(&self) {
        let mut file = lock(&self.file);
        let current = FileStamp::of(&self.path);
        if current == file.stamp {
            file.settling = None;
            return;
        }
        if file.settling != Some(current) {
            // Changed: give the writer time to finish (and see it hold still once more).
            file.settling = Some(current);
            return;
        }
        file.settling = None;
        file.stamp = current;
        if current.is_none() {
            // Deleted or moved away (some editors do that mid-save): keep running; the next
            // save from the app writes it again.
            file.bytes.clear();
            return;
        }
        let Ok(bytes) = fs::read(&self.path) else {
            file.stamp = None;
            return;
        };
        if bytes == file.bytes {
            return;
        }
        file.bytes = bytes;
        let text = String::from_utf8_lossy(&file.bytes).into_owned();
        drop(file);

        let shown = self.path.display();
        match store::parse(&text) {
            Ok((config, repairs)) => {
                if !self.apply(&config) {
                    return;
                }
                log::info!("applied the configuration edited outside the app ({shown})");
                self.emit(RuntimeEvent::Config { config });
                if !repairs.is_empty() {
                    self.emit_error(format!(
                        "The edited configuration in {shown} was loaded with repairs: {}",
                        repairs.join(" ")
                    ));
                }
            }
            Err(error) => {
                self.emit_error(format!(
                    "{shown} has a problem and was not loaded: {error}. The previous \
                     configuration stays active until the file is fixed."
                ));
            }
        }
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
        drop(config);
        // This runs on the engine thread: hand the write to the config watcher thread.
        *lock(&self.save_pending) = true;
        self.save_wake.notify_one();
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
                let connected = connected_inputs(&ports);
                if connected != connected_inputs(&previous) {
                    self.engine.midi_inputs_changed(connected);
                }
                for old in previous.iter().filter(|p| p.state == PortState::Connected) {
                    let still_connected =
                        ports.iter().any(|p| p.name == old.name && p.state == PortState::Connected);
                    if !still_connected {
                        self.engine.device_disconnected(&old.name);
                    }
                }
                self.emit(RuntimeEvent::Ports { ports });
            }
        }
    }

    /// The output ports changed: lights go out again to the ports that came, came back or
    /// changed state (an identical list means one was replaced within a scan and lost what it
    /// showed: then to all of them), and the front end gets the new list.
    fn on_output_event(&self, event: OutputEvent) {
        match event {
            OutputEvent::PortsChanged { ports } => {
                let previous = std::mem::replace(&mut *lock(&self.outputs), ports.clone());
                *lock(&self.output_names) = ports.iter().map(|port| port.name.clone()).collect();
                let replaced = ports == previous;
                let refresh: Vec<String> = ports
                    .iter()
                    .filter(|port| port.state != PortState::Error)
                    .filter(|port| replaced || !previous.contains(port))
                    .map(|port| port.name.clone())
                    .collect();
                // Outputs paired with switched-off inputs may have come or gone.
                let config = lock(&self.config).clone();
                self.update_output_policy(&config);
                if !refresh.is_empty() {
                    self.engine.midi_outputs_changed(refresh);
                }
                self.emit(RuntimeEvent::Outputs { outputs: ports });
            }
        }
    }

    fn rebuild_injector(&self, config: &Config) {
        let (injector, error) = make_injector(config, self.dry_run);
        *lock(&self.unsupported_keys) = unsupported_keys(injector.as_ref());
        self.engine.set_injector(injector);
        *lock(&self.backend_error) = error;
        self.publish_permissions();
    }

    fn publish_permissions(&self) {
        self.emit(RuntimeEvent::Permissions {
            permissions: lock(&self.permissions).clone(),
            backend_error: lock(&self.backend_error).clone(),
            unsupported_keys: lock(&self.unsupported_keys).clone(),
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
                *lock(&self.unsupported_keys) = unsupported_keys(injector.as_ref());
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
