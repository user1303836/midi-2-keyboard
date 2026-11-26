//! MIDI input device management on top of `midir`: enumeration, connections, hot-plug.
//!
//! CONTRACT (types and signatures are relied upon by the app runtime; keep them stable):
//!
//! - [`InputManager::start`] spawns a background thread that keeps the set of open ports in sync
//!   with the [`PortPolicy`]: it opens ports as they appear, notices ports that disappear, retries
//!   ports that failed to open, and reports every change through [`InputEvent::PortsChanged`].
//! - Every received message is decoded and delivered through [`InputEvent::Message`] from the
//!   OS MIDI thread, with a monotonic timestamp. The sink must be fast and must not block.
//! - Port names are **normalized to be stable across reconnects/reboots** (for example the ALSA
//!   `client:port` numbers are stripped) because configs refer to devices by name. When several
//!   identical devices are connected, later ones get a ` #2`, ` #3`, ... suffix.
//! - Dropping the manager closes all ports and stops the thread.
//!
//! # How it works
//!
//! One background thread ("m2k-midi-input") owns every connection. It re-enumerates the ports
//! every `poll_interval_ms` (or right away after [`InputManager::rescan`] /
//! [`InputManager::set_policy`]), opens what the policy allows, closes what it no longer allows
//! or what vanished, retries ports in the `Error` state, and emits `PortsChanged` whenever the
//! list differs from the last one it reported. `midir`'s `connect` consumes its `MidiInput`, so
//! every open port has its own `MidiInput` (one OS MIDI client each).
//!
//! Each connection feeds the received bytes through its own streaming [`Parser`]: WinMM delivers
//! SysEx longer than 1 KiB in several callbacks, and the parser also enforces the SysEx size cap.
//! A port that disappears mid-SysEx simply drops its partial message with the connection.
//! Timestamps come from one [`Instant`] captured in [`InputManager::start`], read when the OS
//! callback runs (midir's own timestamps have a different epoch per port, and only millisecond
//! resolution on Windows). The sink is called with no lock held.
//!
//! # Port names
//!
//! - **Windows (WinMM):** names are used as the OS reports them. WinMM truncates them to 31
//!   characters and already disambiguates identical devices (`2- Launchkey`), so the ` #n` suffix
//!   is only needed when truncation makes two names identical. Ports whose driver does not
//!   answer the device-interface query are not listed by midir at all.
//! - **Linux (ALSA):** midir reports `client:port 20:0`; the trailing sequencer address changes
//!   whenever a device is re-plugged, so it is stripped (`Launchkey MK3 25:Launchkey MK3 25 MIDI 1`).
//! - **macOS (CoreMIDI):** the endpoint display name, as shown in Audio MIDI Setup.
//!
//! Identical devices are told apart by a ` #2`, ` #3`, ... suffix. A name is kept for as long as
//! its port stays present; new ports get the lowest free suffix, and ports appearing in the same
//! scan are numbered in the order of their OS identifier (ALSA address, WinMM interface path,
//! CoreMIDI unique id). Identical devices have no reliable identity, so after a restart the
//! numbering can differ if they are enumerated differently (for example plugged into other ports).
//!
//! # Platform notes
//!
//! - **Windows:** many WinMM drivers allow one client at a time. Opening such a port while
//!   another program has it fails with an actionable `error`; it is retried on every poll, so it
//!   connects as soon as the other program lets go. A device unplugged and re-plugged within a
//!   single poll interval may go unnoticed (WinMM gives no way to tell the new device instance
//!   from the old one); use a shorter `poll_interval_ms` if that matters.
//! - **macOS:** CoreMIDI delivers setup changes (and refreshes the process's port list) on the run
//!   loop of the thread that created the *first* MIDI client of the process. If that run loop
//!   never runs, hot-plugged devices never appear. [`InputManager::start`] therefore creates that
//!   first client on a dedicated, never-exiting thread ("m2k-coremidi") that keeps its run loop
//!   running, so hot-plug works no matter which thread starts managers, and keeps working if a
//!   manager is dropped and started again.
//! - **macOS / Linux virtual port:** with [`InputOptions::virtual_port`], other applications can
//!   send to us. It is listed with `is_virtual: true` and obeys the policy like any port (disabled
//!   = removed from the system). Enumerated ports that are our own virtual port are never opened
//!   (that would be a feedback loop).
//! - midir's ALSA and CoreMIDI backends assemble SysEx split over several OS packets themselves,
//!   and do so without a size limit: a device that starts a SysEx and never ends it can make that
//!   port swallow everything that follows. Our parser's cap applies to what midir hands over.

use std::cmp::Ordering;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use midir::{ConnectErrorKind, Ignore, MidiInput, MidiInputConnection, MidiInputPort};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::message::MidiMessage;
use crate::parser::Parser;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum PortState {
    /// Open and receiving.
    Connected,
    /// Present on the system but not opened because of the policy.
    Disabled,
    /// Present but could not be opened (see `error`); retried automatically.
    Error,
}

/// A MIDI input port currently present on the system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PortStatus {
    /// Normalized, stable port name (what configs store).
    pub name: String,
    pub state: PortState,
    /// Why opening failed; empty unless `state` is `Error`. Should be actionable, e.g. on Windows
    /// "in use by another application" for single-client drivers.
    pub error: String,
    /// `true` for the app's own virtual input port (macOS / Linux).
    pub is_virtual: bool,
}

/// Which ports to open.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum PortPolicy {
    /// Every port.
    #[default]
    All,
    /// Every port except the named ones.
    AllExcept(Vec<String>),
    /// Only the named ports.
    Only(Vec<String>),
}

impl PortPolicy {
    /// Whether the port with this (normalized) name should be open.
    pub fn allows(&self, name: &str) -> bool {
        match self {
            PortPolicy::All => true,
            PortPolicy::AllExcept(names) => !names.iter().any(|n| n == name),
            PortPolicy::Only(names) => names.iter().any(|n| n == name),
        }
    }
}

#[derive(Debug, Clone)]
pub struct InputOptions {
    /// Client name shown to the OS MIDI system.
    pub client_name: String,
    /// How often the port list is re-scanned for hot-plug detection.
    pub poll_interval_ms: u32,
    /// Create a virtual input port with this name so other applications can send MIDI to us
    /// (macOS CoreMIDI / Linux ALSA only; ignored on Windows).
    pub virtual_port: Option<String>,
}

impl Default for InputOptions {
    fn default() -> Self {
        InputOptions {
            client_name: "midi-2-keyboard".into(),
            poll_interval_ms: 1000,
            virtual_port: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum InputEvent {
    /// A message arrived on `port`. `time_us` is microseconds on a monotonic clock shared by all
    /// ports (time since the manager started).
    Message { port: Arc<str>, time_us: u64, message: MidiMessage },
    /// The set of present ports or one of their states changed. Carries the full current list.
    PortsChanged { ports: Vec<PortStatus> },
}

/// Owns all MIDI input connections.
pub struct InputManager {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

/// Shortest accepted `poll_interval_ms`.
const MIN_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long [`InputManager::start`] waits for the first scan, so `ports()` is meaningful
/// right after it returns.
const FIRST_SCAN_TIMEOUT: Duration = Duration::from_secs(3);

impl InputManager {
    /// Starts the manager. `sink` is called from background threads.
    ///
    /// Returns after the first port scan has completed (or after a few seconds if the OS is
    /// unusually slow), so [`ports`](Self::ports) already reflects the system. The first
    /// `PortsChanged` event is sent from that scan, possibly before this returns.
    pub fn start(
        options: InputOptions,
        policy: PortPolicy,
        sink: impl Fn(InputEvent) + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            delivery: Arc::new(Delivery { sink: Box::new(sink), epoch: Instant::now() }),
            control: Mutex::new(Control { policy, scan_requested: false, stop: false }),
            wake: Condvar::new(),
            ports: Mutex::new(Vec::new()),
        });

        #[cfg(target_os = "macos")]
        macos::ensure_notification_thread(&options.client_name);

        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new().name("m2k-midi-input".into()).spawn({
            let shared = shared.clone();
            move || poll_thread(shared, options, ready_tx)
        });
        let thread = match spawned {
            Ok(thread) => {
                let _ = ready_rx.recv_timeout(FIRST_SCAN_TIMEOUT);
                Some(thread)
            }
            Err(error) => {
                log::error!("could not start the MIDI input thread: {error}");
                None
            }
        };
        InputManager { shared, thread }
    }

    /// Replaces the policy; ports are opened/closed accordingly right away.
    ///
    /// Asynchronous: the background thread applies it immediately and reports the result with
    /// `PortsChanged`.
    pub fn set_policy(&self, policy: PortPolicy) {
        let mut control = lock(&self.shared.control);
        control.policy = policy;
        control.scan_requested = true;
        drop(control);
        self.shared.wake.notify_all();
    }

    /// Re-scans the system's ports immediately instead of waiting for the next poll.
    /// Asynchronous, like [`set_policy`](Self::set_policy).
    pub fn rescan(&self) {
        lock(&self.shared.control).scan_requested = true;
        self.shared.wake.notify_all();
    }

    /// Current list of present ports.
    pub fn ports(&self) -> Vec<PortStatus> {
        lock(&self.shared.ports).clone()
    }

    /// Delivers `bytes` through the sink as if they had been received on `port` (the port does
    /// not need to exist). Used by the UI's "simulate MIDI" tools and the CLI.
    ///
    /// The bytes may hold any number of messages. Each call is parsed on its own (running status
    /// does not carry over between calls); a message left incomplete at the end is delivered as
    /// [`MidiMessage::Unknown`]. The sink is called on the calling thread.
    pub fn inject(&self, port: &str, bytes: &[u8]) {
        let delivery = &self.shared.delivery;
        let port: Arc<str> = Arc::from(port);
        let time_us = delivery.now_us();
        let mut emit = |message: MidiMessage| {
            delivery.send(InputEvent::Message { port: port.clone(), time_us, message });
        };
        let mut parser = Parser::new();
        parser.feed(bytes, &mut emit);
        parser.finish(&mut emit);
    }
}

impl Drop for InputManager {
    fn drop(&mut self) {
        lock(&self.shared.control).stop = true;
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            // Dropped from inside the sink on the input thread itself: it exits on its own.
            if thread.thread().id() != std::thread::current().id() && thread.join().is_err() {
                log::error!("the MIDI input thread panicked");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------------------------

type Sink = dyn Fn(InputEvent) + Send + Sync;

/// What the OS callbacks need: the sink and the clock.
struct Delivery {
    sink: Box<Sink>,
    epoch: Instant,
}

impl Delivery {
    fn now_us(&self) -> u64 {
        self.epoch.elapsed().as_micros() as u64
    }

    /// Calls the sink. A panicking sink must not unwind into an OS callback (that aborts the
    /// process) or kill the input thread.
    fn send(&self, event: InputEvent) {
        if catch_unwind(AssertUnwindSafe(|| (self.sink)(event))).is_err() {
            log::error!("the MIDI input event sink panicked");
        }
    }
}

struct Shared {
    delivery: Arc<Delivery>,
    control: Mutex<Control>,
    wake: Condvar,
    /// The last reported port list, for `ports()`.
    ports: Mutex<Vec<PortStatus>>,
}

struct Control {
    policy: PortPolicy,
    /// `rescan()` or `set_policy()` asked for a scan now.
    scan_requested: bool,
    stop: bool,
}

/// Locks a mutex, ignoring poisoning (the protected data stays consistent: every critical
/// section is a plain assignment or clone).
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------------------------
// The input thread
// ---------------------------------------------------------------------------------------------

fn poll_thread(shared: Arc<Shared>, options: InputOptions, ready: mpsc::SyncSender<()>) {
    let interval =
        Duration::from_millis(u64::from(options.poll_interval_ms)).max(MIN_POLL_INTERVAL);
    let mut poller = Poller::new(shared.clone(), options);
    let mut ready = Some(ready);
    loop {
        let policy = {
            let mut control = lock(&shared.control);
            if ready.is_none() {
                let deadline = Instant::now() + interval;
                while !control.stop && !control.scan_requested {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    control = match shared.wake.wait_timeout(control, deadline - now) {
                        Ok((guard, _)) => guard,
                        Err(poisoned) => poisoned.into_inner().0,
                    };
                }
            }
            if control.stop {
                break;
            }
            control.scan_requested = false;
            control.policy.clone()
        };
        if catch_unwind(AssertUnwindSafe(|| poller.scan(&policy))).is_err() {
            log::error!("MIDI port scan failed unexpectedly; retrying on the next poll");
        }
        if let Some(ready) = ready.take() {
            let _ = ready.send(());
        }
    }
    poller.close_all();
}

/// A port found by enumeration.
struct Found {
    raw_name: String,
    id: String,
    /// Normalized name, before de-duplication.
    base: String,
    port: MidiInputPort,
}

/// A port we know about (present on the system), and its connection if open.
struct Entry {
    /// What the OS calls it, and its OS identifier: together they identify the port.
    raw_name: String,
    id: String,
    /// The stable, unique name reported to the outside.
    name: Arc<str>,
    state: PortState,
    error: String,
    connection: Option<MidiInputConnection<()>>,
}

impl Entry {
    fn status(&self, is_virtual: bool) -> PortStatus {
        PortStatus {
            name: self.name.to_string(),
            state: self.state,
            error: self.error.clone(),
            is_virtual,
        }
    }

    /// Records the outcome of an attempt to open the port.
    fn opened(&mut self, result: Result<MidiInputConnection<()>, String>) {
        match result {
            Ok(connection) => {
                log::info!("MIDI input connected: {}", self.name);
                self.connection = Some(connection);
                self.state = PortState::Connected;
                self.error.clear();
            }
            Err(error) => {
                if self.state != PortState::Error || self.error != error {
                    log::warn!("MIDI input {}: {error}", self.name);
                }
                self.state = PortState::Error;
                self.error = error;
            }
        }
    }

    fn disable(&mut self) {
        if self.connection.take().is_some() {
            log::info!("MIDI input closed: {}", self.name);
        }
        self.state = PortState::Disabled;
        self.error.clear();
    }
}

struct Poller {
    shared: Arc<Shared>,
    client_name: String,
    /// Our own virtual port's name (macOS / Linux only).
    virtual_name: Option<String>,
    /// The client used to enumerate ports (kept for the lifetime of the thread).
    enumerator: Option<MidiInput>,
    init_error_logged: bool,
    clients: Clients,
    entries: Vec<Entry>,
    virtual_port: Option<Entry>,
    reported: Option<Vec<PortStatus>>,
}

impl Poller {
    fn new(shared: Arc<Shared>, options: InputOptions) -> Self {
        let virtual_name = options
            .virtual_port
            .map(|name| name.trim().to_owned())
            .filter(|name| cfg!(unix) && !name.is_empty());
        Poller {
            shared,
            clients: Clients { name: options.client_name.clone(), idle: Vec::new() },
            client_name: options.client_name,
            virtual_name,
            enumerator: None,
            init_error_logged: false,
            entries: Vec::new(),
            virtual_port: None,
            reported: None,
        }
    }

    fn scan(&mut self, policy: &PortPolicy) {
        if let Some(found) = self.enumerate() {
            self.update_entries(&found, policy);
        }
        self.update_virtual_port(policy);
        self.report();
    }

    /// The ports present right now, or `None` when the MIDI system cannot be queried (in which
    /// case the known ports are left as they are).
    fn enumerate(&mut self) -> Option<Vec<Found>> {
        if self.enumerator.is_none() {
            match MidiInput::new(&self.client_name) {
                Ok(input) => {
                    self.enumerator = Some(input);
                    self.init_error_logged = false;
                }
                Err(error) => {
                    if !self.init_error_logged {
                        log::warn!("MIDI input unavailable: {error}");
                        self.init_error_logged = true;
                    }
                    return None;
                }
            }
        }
        let input = self.enumerator.as_ref()?;
        let listed = catch_unwind(AssertUnwindSafe(|| {
            input
                .ports()
                .into_iter()
                .filter_map(|port| {
                    let raw_name = input.port_name(&port).ok()?; // vanished meanwhile
                    let base = normalize_port_name(&raw_name);
                    Some(Found { id: port.id(), raw_name, base, port })
                })
                .collect::<Vec<_>>()
        }));
        match listed {
            Ok(found) => Some(found),
            Err(_) => {
                log::error!("enumerating MIDI inputs failed; retrying with a new client");
                self.enumerator = None;
                None
            }
        }
    }

    fn update_entries(&mut self, found: &[Found], policy: &PortPolicy) {
        let is_present =
            |entry: &Entry| found.iter().any(|f| f.raw_name == entry.raw_name && f.id == entry.id);
        // Vanished ports: dropping the entry closes its connection.
        self.entries.retain(|entry| {
            let present = is_present(entry);
            if !present {
                log::info!("MIDI input removed: {}", entry.name);
            }
            present
        });

        // New ports, numbered deterministically.
        let mut new: Vec<&Found> = found
            .iter()
            .filter(|f| {
                !self.entries.iter().any(|e| e.raw_name == f.raw_name && e.id == f.id)
                    && !self.is_own_virtual_port(&f.base)
            })
            .collect();
        new.sort_by(|a, b| natural_cmp(&a.base, &b.base).then_with(|| natural_cmp(&a.id, &b.id)));
        new.dedup_by(|a, b| a.raw_name == b.raw_name && a.id == b.id);
        for f in new {
            let name = self.unique_name(&f.base);
            log::info!("MIDI input found: {name}");
            self.entries.push(Entry {
                raw_name: f.raw_name.clone(),
                id: f.id.clone(),
                name: Arc::from(name),
                state: PortState::Disabled,
                error: String::new(),
                connection: None,
            });
        }

        // Apply the policy: open what is wanted (including retrying failures), close the rest.
        let delivery = &self.shared.delivery;
        for entry in &mut self.entries {
            if !policy.allows(&entry.name) {
                entry.disable();
            } else if entry.connection.is_none() {
                let port = found.iter().find(|f| f.raw_name == entry.raw_name && f.id == entry.id);
                if let Some(found) = port {
                    let result = self.clients.open(delivery, &found.port, &entry.name);
                    entry.opened(result);
                }
            }
        }
    }

    fn update_virtual_port(&mut self, policy: &PortPolicy) {
        let Some(name) = self.virtual_name.clone() else {
            return;
        };
        let entry = self.virtual_port.get_or_insert_with(|| Entry {
            raw_name: name.clone(),
            id: String::new(),
            name: Arc::from(name.as_str()),
            state: PortState::Disabled,
            error: String::new(),
            connection: None,
        });
        if !policy.allows(&entry.name) {
            entry.disable();
        } else if entry.connection.is_none() {
            let result = self.clients.open_virtual(&self.shared.delivery, &entry.name);
            entry.opened(result);
        }
    }

    /// Whether an enumerated port is the virtual port this manager created. On ALSA ports are
    /// listed as `client:port`; on CoreMIDI by the endpoint name.
    fn is_own_virtual_port(&self, base: &str) -> bool {
        self.virtual_name.as_deref().is_some_and(|name| {
            base == name
                || base
                    .strip_prefix(self.client_name.as_str())
                    .and_then(|rest| rest.strip_prefix(':'))
                    .is_some_and(|rest| rest == name)
        })
    }

    /// `base`, or `base #2`, `base #3`, ... whichever is not taken yet.
    fn unique_name(&self, base: &str) -> String {
        let taken = |candidate: &str| {
            self.entries.iter().any(|e| &*e.name == candidate)
                || self.virtual_name.as_deref() == Some(candidate)
        };
        if !taken(base) {
            return base.to_owned();
        }
        let mut n = 2u32;
        loop {
            let candidate = format!("{base} #{n}");
            if !taken(&candidate) {
                return candidate;
            }
            n += 1;
        }
    }

    /// Publishes the port list and emits `PortsChanged` if it differs from the last report.
    fn report(&mut self) {
        let mut ports: Vec<PortStatus> = self
            .entries
            .iter()
            .map(|e| e.status(false))
            .chain(self.virtual_port.iter().map(|e| e.status(true)))
            .collect();
        ports.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        if self.reported.as_ref() == Some(&ports) {
            return;
        }
        *lock(&self.shared.ports) = ports.clone();
        self.reported = Some(ports.clone());
        self.shared.delivery.send(InputEvent::PortsChanged { ports });
    }

    fn close_all(&mut self) {
        self.entries.clear();
        self.virtual_port = None;
        self.enumerator = None;
        self.clients.idle.clear();
    }
}

/// The callback of one connection: decode and deliver.
fn port_callback(
    delivery: Arc<Delivery>,
    port: Arc<str>,
) -> impl FnMut(u64, &[u8], &mut ()) + Send + 'static {
    let mut parser = Parser::new();
    move |_os_time, bytes, _| {
        let time_us = delivery.now_us();
        parser.feed(bytes, &mut |message| {
            delivery.send(InputEvent::Message { port: port.clone(), time_us, message });
        });
    }
}

/// OS MIDI clients for new connections: midir's `connect` consumes one client per open port.
///
/// Clients handed back by failed attempts are reused by the next attempt. That matters on macOS,
/// where a CoreMIDI client is never freed (coremidi deliberately does not dispose clients:
/// disposing the last one can make the MIDI server exit), so a port retried every poll would
/// otherwise leak a client per retry. Closing a connection drops it, which on macOS leaks its
/// client too; that is bounded by how often ports are closed or unplugged. (midir's
/// `MidiInputConnection::close` would return the client, but its CoreMIDI version lets a packet
/// arriving during the close panic inside the CoreMIDI callback, which aborts the process.)
struct Clients {
    name: String,
    idle: Vec<MidiInput>,
}

impl Clients {
    /// A client that receives everything (SysEx, clock, active sensing).
    fn take(&mut self) -> Result<MidiInput, String> {
        let mut input = match self.idle.pop() {
            Some(input) => input,
            None => MidiInput::new(&self.name)
                .map_err(|e| format!("The MIDI system is not available ({e})."))?,
        };
        input.ignore(Ignore::None);
        Ok(input)
    }

    fn open(
        &mut self,
        delivery: &Arc<Delivery>,
        port: &MidiInputPort,
        name: &Arc<str>,
    ) -> Result<MidiInputConnection<()>, String> {
        let input = self.take()?;
        let callback = port_callback(delivery.clone(), name.clone());
        // midir names the connection's own port after this (visible in e.g. `aconnect -l`).
        let connection_name = name.replace('\0', "");
        match catch_unwind(AssertUnwindSafe(|| input.connect(port, &connection_name, callback, ())))
        {
            Ok(Ok(connection)) => Ok(connection),
            Ok(Err(error)) => {
                let message = describe_connect_error(error.kind());
                self.idle.push(error.into_inner());
                Err(message)
            }
            Err(_) => Err("The MIDI driver failed unexpectedly while opening the port.".into()),
        }
    }

    #[cfg(unix)]
    fn open_virtual(
        &mut self,
        delivery: &Arc<Delivery>,
        name: &Arc<str>,
    ) -> Result<MidiInputConnection<()>, String> {
        use midir::ext::VirtualInput;

        let input = self.take()?;
        let callback = port_callback(delivery.clone(), name.clone());
        let port_name = name.replace('\0', "");
        match catch_unwind(AssertUnwindSafe(|| input.create_virtual(&port_name, callback, ()))) {
            Ok(Ok(connection)) => Ok(connection),
            Ok(Err(error)) => {
                let message = format!("Could not create the virtual port: {}.", error.kind());
                self.idle.push(error.into_inner());
                Err(message)
            }
            Err(_) => {
                Err("The MIDI system failed unexpectedly while creating the virtual port.".into())
            }
        }
    }

    #[cfg(not(unix))]
    fn open_virtual(
        &mut self,
        _delivery: &Arc<Delivery>,
        _name: &Arc<str>,
    ) -> Result<MidiInputConnection<()>, String> {
        Err("Virtual ports are not supported on this platform.".into())
    }
}

/// Turns a midir connect error into something the user can act on.
///
/// On Windows a failing `midiInOpen` practically always means another program has the port
/// open: most WinMM drivers, including the built-in USB MIDI class driver, accept one client at a
/// time. Verified with a class-compliant controller: a second open from another process fails
/// with `MMSYSERR_NOMEM`, one from the same process with `MMSYSERR_ALLOCATED`. midir reports the
/// latter by name and every other code as the same generic message, hence the prefix match.
fn describe_connect_error(kind: ConnectErrorKind) -> String {
    const WINMM_OPEN_FAILED: &str = "could not create Windows MM MIDI input port";
    match kind {
        ConnectErrorKind::InvalidPort => "The port disappeared while it was being opened.".into(),
        ConnectErrorKind::Other(message) if message.contains("MMSYSERR_ALLOCATED") => {
            "In use by another application (this Windows MIDI driver allows only one program at \
             a time). Close the other program, for example a DAW or another MIDI tool, and the \
             port connects automatically."
                .into()
        }
        ConnectErrorKind::Other(message) if message.starts_with(WINMM_OPEN_FAILED) => {
            "Could not open the port: it is probably in use by another application (most Windows \
             MIDI drivers allow only one program at a time). Close the other program, for example \
             a DAW or another MIDI tool, and the port connects automatically."
                .into()
        }
        ConnectErrorKind::Other(message) => format!("Could not open the port: {message}."),
    }
}

/// Makes an OS port name stable across reconnects and reboots (see the module docs).
fn normalize_port_name(raw: &str) -> String {
    let mut name = raw.trim();
    if cfg!(target_os = "linux") {
        name = strip_alsa_address(name);
    }
    if name.is_empty() { "Unnamed MIDI input".to_owned() } else { name.to_owned() }
}

/// `"Launchkey:Launchkey MIDI 1 20:0"` -> `"Launchkey:Launchkey MIDI 1"`.
fn strip_alsa_address(name: &str) -> &str {
    let is_number = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    match name.rsplit_once(' ') {
        Some((head, address))
            if address.split_once(':').is_some_and(|(c, p)| is_number(c) && is_number(p)) =>
        {
            head.trim_end()
        }
        _ => name,
    }
}

/// Case-insensitive ordering that compares digit runs by value (`Port 2` < `Port 10`), with a
/// plain byte comparison as the final tie-break so the order is total.
fn natural_cmp(a: &str, b: &str) -> Ordering {
    let (mut x, mut y) = (a.as_bytes(), b.as_bytes());
    loop {
        match (x.first(), y.first()) {
            (None, None) => return a.cmp(b),
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(cx), Some(cy)) if cx.is_ascii_digit() && cy.is_ascii_digit() => {
                let lx = x.iter().take_while(|c| c.is_ascii_digit()).count();
                let ly = y.iter().take_while(|c| c.is_ascii_digit()).count();
                let (nx, ny) = (trim_zeros(&x[..lx]), trim_zeros(&y[..ly]));
                let order = nx.len().cmp(&ny.len()).then_with(|| nx.cmp(ny));
                if order != Ordering::Equal {
                    return order;
                }
                x = &x[lx..];
                y = &y[ly..];
            }
            (Some(cx), Some(cy)) => {
                let order = cx.to_ascii_lowercase().cmp(&cy.to_ascii_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                x = &x[1..];
                y = &y[1..];
            }
        }
    }
}

fn trim_zeros(digits: &[u8]) -> &[u8] {
    let zeros = digits.iter().take_while(|&&c| c == b'0').count();
    &digits[zeros..]
}

#[cfg(target_os = "macos")]
mod macos {
    //! See "Platform notes" in the module docs: the first CoreMIDI client of the process binds
    //! CoreMIDI's notifications to its thread's run loop, so it lives on a dedicated thread that
    //! keeps that run loop running for the rest of the process.

    use std::ffi::c_void;
    use std::sync::OnceLock;
    use std::time::Duration;

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        static kCFRunLoopDefaultMode: *const c_void;
        fn CFRunLoopRunInMode(mode: *const c_void, seconds: f64, return_after_source: u8) -> i32;
    }

    /// `kCFRunLoopRunFinished`: the run loop has no sources (nothing to wait for).
    const RUN_LOOP_FINISHED: i32 = 1;

    pub fn ensure_notification_thread(client_name: &str) {
        static STARTED: OnceLock<()> = OnceLock::new();
        STARTED.get_or_init(|| {
            let client_name = client_name.to_owned();
            let (created_tx, created_rx) = std::sync::mpsc::channel::<()>();
            let spawned =
                std::thread::Builder::new().name("m2k-coremidi".into()).spawn(move || {
                    // Kept alive for the rest of the process: the loop below never ends.
                    let _client = midir::MidiInput::new(&client_name);
                    let _ = created_tx.send(());
                    loop {
                        let result = unsafe { CFRunLoopRunInMode(kCFRunLoopDefaultMode, 1.0, 0) };
                        if result == RUN_LOOP_FINISHED {
                            std::thread::sleep(Duration::from_millis(500));
                        }
                    }
                });
            match spawned {
                Ok(_) => {
                    let _ = created_rx.recv_timeout(Duration::from_secs(5));
                }
                Err(error) => {
                    log::error!("could not start the CoreMIDI notification thread: {error}")
                }
            }
        });
    }
}
