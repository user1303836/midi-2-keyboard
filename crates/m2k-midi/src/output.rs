//! MIDI output device management on top of `midir`: enumeration, hot-plug, lazily opened
//! connections and a send queue that never blocks the caller.
//!
//! CONTRACT (types and signatures are relied upon by the engine and the app runtime; keep them
//! stable):
//!
//! - [`OutputManager::start`] spawns a background thread ("m2k-midi-output") that keeps the list
//!   of output ports in sync with the system (polled like the inputs) and reports every change
//!   through [`OutputEvent::PortsChanged`] with the full current list of [`PortStatus`].
//! - Output names are made exactly like input names (same normalization, same ` #2`, ` #3`, ...
//!   suffixes; see [`crate::input`]), so a device whose input and output the OS names alike (for
//!   example `Faderfox EC4`) carries the same name on both sides. That is how the app pairs them;
//!   [`paired_output_name`] also pairs the extra ports Windows names `MIDIINn (…)` and
//!   `MIDIOUTn (…)`. Two identical controllers are numbered in the order of their OS identifier
//!   on both sides; on Windows (one device interface for input and output) and on Linux (one
//!   ALSA port) that order is the same, but on macOS sources and destinations have unrelated ids,
//!   so `X #2` in and out can belong to different units there.
//! - Ports are opened **lazily**: the first [`send`](OutputManager::send) to a port opens it, and
//!   from then on it is kept open. Ports nobody sends to are never opened (many Windows drivers
//!   take one program at a time). A port that vanishes is closed, and opened again as soon as it
//!   returns. A failed open is retried on every poll for as long as the port is wanted.
//!   [`PortState`] means: `Connected` = open; `Disabled` = present but not open (nothing was sent
//!   to it yet, or the [`PortPolicy`] switches it off); `Error` = opening or sending failed (see
//!   `error`).
//! - A port that went away and came back is always reported, even when the list looks exactly
//!   as it did before (the device has lost whatever it was showing, for example its lights).
//! - [`send`](OutputManager::send) never blocks. It checks the bytes ([`split_messages`]) and
//!   the port and queues them for the output thread; a port must always be named. When the queue
//!   is full, the OLDEST queued send is dropped and counted ([`dropped`](OutputManager::dropped)).
//!   Sends go out in the order they were made. Sends to a port that is in the `Error` state are
//!   dropped (not counted) until a poll manages to open it again.
//! - Dropping the manager first delivers what is still queued (for at most [`FLUSH_ON_DROP`]),
//!   then closes every port and stops the thread.
//!
//! # How it works
//!
//! One thread owns every connection. It waits for sends, a rescan request or the next poll;
//! on a poll it re-enumerates the ports, closes those that vanished, adds new ones (named the
//! same deterministic way as inputs), opens the wanted ones that are closed (new arrivals of a
//! port we sent to before, ports in the `Error` state) and reports changes. A send takes the
//! whole queue at once and hands each message to `midir` separately: midir's ALSA backend sends
//! only the first event of what it is given and its WinMM backend refuses anything but a single
//! message, so [`split_messages`] cuts the bytes into complete messages first (expanding running
//! status). The sink is called on the output thread; it must not call
//! [`flush`](OutputManager::flush) or drop the manager.
//!
//! # Platform notes
//!
//! - **Windows:** "Microsoft GS Wavetable Synth" (a software synthesizer on the speakers) is
//!   always listed. Single-client drivers refuse a port another program holds; the port is then
//!   in the `Error` state with advice, and retried every poll. midir's WinMM backend sends a SysEx
//!   synchronously: the output thread waits until the driver has sent it (at MIDI speed,
//!   3125 bytes/s on a DIN cable) while other sends wait in the queue. It gives up after about
//!   two seconds and then frees a buffer the driver may still be reading, hence the
//!   [`MAX_SYSEX_LEN`] cap (about 1.3 s at MIDI speed).
//! - **Linux:** ports of ALSA clients named like ours are never listed (they are our own
//!   connections, including the input side's, see [`crate::input`]); use the same `client_name`
//!   on both sides (the default).
//! - **macOS / Linux:** the input side's virtual port is a destination other programs send to,
//!   so it shows up as an output; name it in [`OutputOptions::own_virtual_input`] and it is
//!   left out (sending to it would loop straight back in). CoreMIDI hot-plug needs the same
//!   notification thread as the input side, which [`OutputManager::start`] makes sure of.

use std::collections::{HashSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use midir::{ConnectErrorKind, MidiOutput, MidiOutputConnection, MidiOutputPort};

use crate::input::{
    PortPolicy, PortState, PortStatus, SHARING_ADVICE, is_own_client_port, lock, natural_cmp,
    normalize_port_name,
};

/// The most bytes one [`OutputManager::send`] may carry.
pub const MAX_SEND_LEN: usize = 64 * 1024;

/// The longest single SysEx message (`F0` ... `F7`) that may be sent; see the platform notes.
pub const MAX_SYSEX_LEN: usize = 4096;

/// How long dropping the manager keeps delivering what is still queued.
pub const FLUSH_ON_DROP: Duration = Duration::from_secs(2);

/// Bytes allowed to wait in the queue at once (besides the count limit), so a flood of large
/// sends cannot pile up memory.
const MAX_QUEUED_BYTES: usize = 1 << 20;

/// Shortest accepted `poll_interval_ms`.
const MIN_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long [`OutputManager::start`] waits for the first scan.
const FIRST_SCAN_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone)]
pub struct OutputOptions {
    /// Client name shown to the OS MIDI system. Use the same one as the input side (the
    /// default): on Linux each side leaves out the ports of clients with its own name.
    pub client_name: String,
    /// How often the port list is re-scanned for hot-plug detection.
    pub poll_interval_ms: u32,
    /// How many sends may wait for the output thread before the oldest is dropped (at least 1).
    pub queue_capacity: usize,
    /// The input side's virtual port (`InputOptions::virtual_port`), which other programs see as
    /// an output: it is left out of the list. macOS / Linux; ignored on Windows.
    pub own_virtual_input: Option<String>,
}

impl Default for OutputOptions {
    fn default() -> Self {
        OutputOptions {
            client_name: "midi-2-keyboard".into(),
            poll_interval_ms: 1000,
            queue_capacity: 1024,
            own_virtual_input: None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum OutputEvent {
    /// The set of present output ports or one of their states changed. Carries the full list.
    PortsChanged { ports: Vec<PortStatus> },
}

/// Why [`OutputManager::send`] refused a send. Nothing was queued.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SendError {
    /// `port` was `None`: sends always name their port.
    #[error("no MIDI output was named")]
    NoPort,
    /// No output port of that name is present right now.
    #[error("there is no MIDI output called “{0}” right now")]
    UnknownPort(String),
    /// The port is present but the policy switches it off.
    #[error("the MIDI output “{0}” is switched off")]
    Disabled(String),
    /// The bytes are not valid MIDI.
    #[error(transparent)]
    Invalid(#[from] InvalidMidi),
    /// The manager is shutting down or its thread is not running.
    #[error("MIDI output is not running")]
    Stopped,
}

/// What is wrong with bytes given to [`split_messages`]. Offsets count from 0.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum InvalidMidi {
    #[error("there are no bytes to send")]
    Empty,
    #[error("{len} bytes are more than the {max} one send may carry")]
    TooLong { len: usize, max: usize },
    #[error("byte {offset} ({byte:02X}) is a data byte where a status byte (80-FF) has to come")]
    MissingStatus { offset: usize, byte: u8 },
    #[error("byte {offset} ({byte:02X}) is not a defined MIDI status byte")]
    UndefinedStatus { offset: usize, byte: u8 },
    #[error("the message starting at byte {start} is incomplete (it ends at byte {offset})")]
    Incomplete { start: usize, offset: usize },
    #[error("the SysEx starting at byte {start} has no F7 at its end")]
    UnterminatedSysEx { start: usize },
    #[error("the SysEx starting at byte {start} is interrupted by a status byte at byte {offset}")]
    InterruptedSysEx { start: usize, offset: usize },
    #[error("the SysEx starting at byte {start} is {len} bytes long; at most {max} can be sent")]
    SysExTooLong { start: usize, len: usize, max: usize },
    #[error("byte {offset} (F7) ends a SysEx that never started")]
    StrayEndOfSysEx { offset: usize },
}

/// Cuts `bytes` into complete MIDI messages, ready to be sent one by one, or says what is wrong.
///
/// Accepted: channel messages (with running status: `B0 07 64 0A 40` is two Control Changes,
/// returned as `B0 07 64` and `B0 0A 40`), SysEx (`F0` ... `F7`, at most [`MAX_SYSEX_LEN`] bytes,
/// nothing but data bytes inside), system common and real-time messages. Refused: anything else,
/// including messages cut short and undefined status bytes.
pub fn split_messages(bytes: &[u8]) -> Result<Vec<Vec<u8>>, InvalidMidi> {
    if bytes.is_empty() {
        return Err(InvalidMidi::Empty);
    }
    if bytes.len() > MAX_SEND_LEN {
        return Err(InvalidMidi::TooLong { len: bytes.len(), max: MAX_SEND_LEN });
    }
    let mut messages = Vec::new();
    // Status of the last channel message, for running status. System common messages and SysEx
    // cancel it; real-time messages do not.
    let mut running: Option<u8> = None;
    let mut at = 0;
    while at < bytes.len() {
        let start = at;
        let byte = bytes[at];
        if byte == 0xF0 {
            let len = match bytes[at + 1..].iter().position(|&b| b >= 0x80) {
                None => return Err(InvalidMidi::UnterminatedSysEx { start }),
                Some(end) if bytes[at + 1 + end] != 0xF7 => {
                    return Err(InvalidMidi::InterruptedSysEx { start, offset: at + 1 + end });
                }
                Some(end) => end + 2,
            };
            if len > MAX_SYSEX_LEN {
                return Err(InvalidMidi::SysExTooLong { start, len, max: MAX_SYSEX_LEN });
            }
            messages.push(bytes[at..at + len].to_vec());
            running = None;
            at += len;
            continue;
        }
        let (status, data_at) = if byte >= 0x80 {
            (byte, at + 1)
        } else if let Some(status) = running {
            (status, at)
        } else {
            return Err(InvalidMidi::MissingStatus { offset: at, byte });
        };
        let data_len = match status {
            0x80..=0xBF | 0xE0..=0xEF | 0xF2 => 2,
            0xC0..=0xDF | 0xF1 | 0xF3 => 1,
            0xF6 | 0xF8 | 0xFA..=0xFC | 0xFE | 0xFF => 0,
            0xF7 => return Err(InvalidMidi::StrayEndOfSysEx { offset: at }),
            _ => return Err(InvalidMidi::UndefinedStatus { offset: at, byte: status }),
        };
        for offset in data_at..data_at + data_len {
            match bytes.get(offset) {
                Some(&b) if b < 0x80 => {}
                Some(_) => return Err(InvalidMidi::Incomplete { start, offset }),
                None => return Err(InvalidMidi::Incomplete { start, offset: bytes.len() }),
            }
        }
        let mut message = Vec::with_capacity(1 + data_len);
        message.push(status);
        message.extend_from_slice(&bytes[data_at..data_at + data_len]);
        messages.push(message);
        match status {
            0x80..=0xEF => running = Some(status),
            0xF1..=0xF6 => running = None,
            _ => {} // real-time
        }
        at = data_at + data_len;
    }
    Ok(messages)
}

/// The output that goes with the input port called `input`, among `outputs` (port names as
/// reported by both managers): the output with the same name, or, for the extra ports Windows
/// gives a multi-port USB device, the `MIDIOUTn` of the same number for a `MIDIINn` (the names
/// are cut at different points: `MIDIIN2 (Launchkey Mini MK3` pairs with
/// `MIDIOUT2 (Launchkey Mini MK`). A ` #n` suffix has to match too. `None` when no output, or
/// more than one, fits.
pub fn paired_output_name<'a>(
    input: &str,
    outputs: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    let outputs: Vec<&'a str> = outputs.into_iter().collect();
    if let Some(same) = outputs.iter().find(|&&name| name == input) {
        return Some(same);
    }
    let (input_core, input_suffix) = split_suffix(input);
    let (cable, input_rest) = numbered_port(input_core, "MIDIIN")?;
    let mut fitting = outputs.iter().copied().filter(|&name| {
        let (core, suffix) = split_suffix(name);
        suffix == input_suffix
            && numbered_port(core, "MIDIOUT").is_some_and(|(n, rest)| {
                n == cable && input_rest.chars().zip(rest.chars()).all(|(a, b)| a == b)
            })
    });
    let first = fitting.next()?;
    fitting.next().is_none().then_some(first)
}

/// `"Name #3"` -> `("Name", Some("3"))`, `"Name"` -> `("Name", None)`.
fn split_suffix(name: &str) -> (&str, Option<&str>) {
    match name.rsplit_once(" #") {
        Some((core, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => {
            (core, Some(n))
        }
        _ => (name, None),
    }
}

/// `"MIDIIN2 (Launchkey"` with `kind` `"MIDIIN"` -> `("2", "Launchkey")`.
fn numbered_port<'s>(name: &'s str, kind: &str) -> Option<(&'s str, &'s str)> {
    let rest = name.strip_prefix(kind)?;
    let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
    let (number, rest) = rest.split_at(digits);
    let rest = rest.strip_prefix(" (")?;
    (digits > 0 && !rest.is_empty()).then_some((number, rest))
}

/// Owns all MIDI output connections.
pub struct OutputManager {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl OutputManager {
    /// Starts the manager. `sink` is called from the output thread.
    ///
    /// Returns after the first port scan has completed (or after a few seconds if the OS is
    /// unusually slow), so [`ports`](Self::ports) already reflects the system. No port is opened
    /// until something is sent to it.
    pub fn start(
        options: OutputOptions,
        policy: PortPolicy,
        sink: impl Fn(OutputEvent) + Send + Sync + 'static,
    ) -> Self {
        let shared = Arc::new(Shared {
            sink: Box::new(sink),
            state: Mutex::new(State {
                policy,
                scan_requested: false,
                stop: false,
                stop_deadline: None,
                alive: true,
                queue: VecDeque::new(),
                queued_bytes: 0,
                capacity: options.queue_capacity.max(1),
                busy: false,
            }),
            wake: Condvar::new(),
            idle: Condvar::new(),
            ports: Mutex::new(Vec::new()),
            dropped: AtomicU64::new(0),
        });

        #[cfg(target_os = "macos")]
        crate::input::macos::ensure_notification_thread(&options.client_name);

        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new().name("m2k-midi-output".into()).spawn({
            let shared = shared.clone();
            move || output_thread(shared, options, ready_tx)
        });
        let thread = match spawned {
            Ok(thread) => {
                let _ = ready_rx.recv_timeout(FIRST_SCAN_TIMEOUT);
                Some(thread)
            }
            Err(error) => {
                log::error!("could not start the MIDI output thread: {error}");
                lock_state(&shared).alive = false;
                None
            }
        };
        OutputManager { shared, thread }
    }

    /// Replaces the policy: ports it switches off are closed right away (and refuse sends);
    /// wanted ports it allows again are reopened. Asynchronous, reported with `PortsChanged`.
    pub fn set_policy(&self, policy: PortPolicy) {
        let mut state = lock_state(&self.shared);
        state.policy = policy;
        state.scan_requested = true;
        drop(state);
        self.shared.wake.notify_all();
    }

    /// Re-scans the system's ports immediately instead of waiting for the next poll.
    pub fn rescan(&self) {
        lock_state(&self.shared).scan_requested = true;
        self.shared.wake.notify_all();
    }

    /// Current list of present output ports.
    pub fn ports(&self) -> Vec<PortStatus> {
        lock(&self.shared.ports).clone()
    }

    /// Queues `bytes` (one or more complete messages, see [`split_messages`]) for `port`, and
    /// returns at once: the output thread opens the port if needed and sends them. `None` is
    /// refused ([`SendError::NoPort`]): there is no "send to every output".
    ///
    /// Errors are about this call only (port name, policy, bytes); whether the port could be
    /// opened shows in its [`PortStatus`].
    pub fn send(&self, port: Option<&str>, bytes: &[u8]) -> Result<(), SendError> {
        let port = port.ok_or(SendError::NoPort)?;
        let messages = split_messages(bytes)?;
        if !lock(&self.shared.ports).iter().any(|p| p.name == port) {
            return Err(SendError::UnknownPort(port.to_owned()));
        }
        let size: usize = messages.iter().map(Vec::len).sum();
        let mut state = lock_state(&self.shared);
        if state.stop || !state.alive {
            return Err(SendError::Stopped);
        }
        if !state.policy.allows(port) {
            return Err(SendError::Disabled(port.to_owned()));
        }
        while state.queue.len() >= state.capacity
            || (!state.queue.is_empty() && state.queued_bytes + size > MAX_QUEUED_BYTES)
        {
            let Some(oldest) = state.queue.pop_front() else { break };
            state.queued_bytes -= oldest.size;
            self.shared.dropped.fetch_add(1, AtomicOrdering::Relaxed);
        }
        state.queued_bytes += size;
        state.queue.push_back(Pending { port: port.to_owned(), messages, size });
        drop(state);
        self.shared.wake.notify_all();
        Ok(())
    }

    /// How many queued sends were dropped because the queue was full (since the start).
    pub fn dropped(&self) -> u64 {
        self.shared.dropped.load(AtomicOrdering::Relaxed)
    }

    /// Waits until everything queued so far has been handed to the OS (or found undeliverable),
    /// for at most `timeout`. Returns whether the queue was emptied. Never call it from the sink.
    pub fn flush(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = lock_state(&self.shared);
        loop {
            if state.queue.is_empty() && !state.busy {
                return true;
            }
            let now = Instant::now();
            if !state.alive || now >= deadline {
                return false;
            }
            state = match self.shared.idle.wait_timeout(state, deadline - now) {
                Ok((guard, _)) => guard,
                Err(poisoned) => poisoned.into_inner().0,
            };
        }
    }
}

impl Drop for OutputManager {
    fn drop(&mut self) {
        {
            let mut state = lock_state(&self.shared);
            state.stop = true;
            state.stop_deadline = Some(Instant::now() + FLUSH_ON_DROP);
        }
        self.shared.wake.notify_all();
        if let Some(thread) = self.thread.take() {
            // Dropped from inside the sink on the output thread itself: it exits on its own.
            if thread.thread().id() != std::thread::current().id() && thread.join().is_err() {
                log::error!("the MIDI output thread panicked");
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------------------------

struct Shared {
    sink: Box<dyn Fn(OutputEvent) + Send + Sync>,
    state: Mutex<State>,
    /// Wakes the output thread: a send, a rescan, a policy change or the stop.
    wake: Condvar,
    /// Wakes `flush()`: the output thread finished a batch.
    idle: Condvar,
    /// The last reported port list, for `ports()` and the checks in `send()`.
    ports: Mutex<Vec<PortStatus>>,
    dropped: AtomicU64,
}

impl Shared {
    /// Calls the sink; a panicking sink must not kill the output thread.
    fn emit(&self, event: OutputEvent) {
        if catch_unwind(AssertUnwindSafe(|| (self.sink)(event))).is_err() {
            log::error!("the MIDI output event sink panicked");
        }
    }
}

/// Everything `send()` and the output thread share. Every critical section is short and does no
/// I/O, so `send()` never waits for the OS.
struct State {
    policy: PortPolicy,
    scan_requested: bool,
    stop: bool,
    /// Until when a stopping thread keeps delivering what is queued.
    stop_deadline: Option<Instant>,
    /// The output thread is running.
    alive: bool,
    queue: VecDeque<Pending>,
    queued_bytes: usize,
    capacity: usize,
    /// The output thread is delivering a batch it took from the queue.
    busy: bool,
}

fn lock_state(shared: &Shared) -> MutexGuard<'_, State> {
    lock(&shared.state)
}

/// One accepted send.
struct Pending {
    port: String,
    messages: Vec<Vec<u8>>,
    size: usize,
}

// ---------------------------------------------------------------------------------------------
// The output thread
// ---------------------------------------------------------------------------------------------

fn output_thread(shared: Arc<Shared>, options: OutputOptions, ready: mpsc::SyncSender<()>) {
    let interval =
        Duration::from_millis(u64::from(options.poll_interval_ms)).max(MIN_POLL_INTERVAL);
    let mut poller = Poller::new(shared.clone(), options);
    let mut ready = Some(ready);
    let mut next_scan = Instant::now();
    loop {
        let (batch, policy, scan, deadline) = {
            let mut state = lock_state(&shared);
            loop {
                let now = Instant::now();
                if state.stop || !state.queue.is_empty() || state.scan_requested || now >= next_scan
                {
                    break;
                }
                state = match shared.wake.wait_timeout(state, next_scan - now) {
                    Ok((guard, _)) => guard,
                    Err(poisoned) => poisoned.into_inner().0,
                };
            }
            let scan = !state.stop && (state.scan_requested || Instant::now() >= next_scan);
            state.scan_requested = false;
            let batch: Vec<Pending> = state.queue.drain(..).collect();
            state.queued_bytes = 0;
            state.busy = !batch.is_empty();
            (batch, state.policy.clone(), scan, state.stop_deadline)
        };

        if scan {
            if catch_unwind(AssertUnwindSafe(|| poller.scan(&policy))).is_err() {
                log::error!("MIDI output port scan failed unexpectedly; retrying on the next poll");
            }
            next_scan = Instant::now() + interval;
            if let Some(ready) = ready.take() {
                let _ = ready.send(());
            }
        }
        for pending in &batch {
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                shared.dropped.fetch_add(1, AtomicOrdering::Relaxed);
                continue;
            }
            if catch_unwind(AssertUnwindSafe(|| poller.deliver(pending, &policy))).is_err() {
                // The connection is in an unknown state now: close it (the next poll opens it
                // again while the port is wanted) and say so on the port.
                poller.failed(&pending.port, "The MIDI driver failed unexpectedly while sending.");
            }
        }
        poller.report();

        let mut state = lock_state(&shared);
        state.busy = false;
        let finished = state.stop && state.queue.is_empty();
        if finished {
            state.alive = false;
        }
        drop(state);
        shared.idle.notify_all();
        if finished {
            break;
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
    port: MidiOutputPort,
}

/// A port present on the system, and its connection if open.
struct Entry {
    raw_name: String,
    id: String,
    /// The stable, unique name reported to the outside.
    name: String,
    /// The OS port, as of the last scan.
    port: MidiOutputPort,
    state: PortState,
    error: String,
    connection: Option<MidiOutputConnection>,
}

impl Entry {
    fn status(&self) -> PortStatus {
        PortStatus {
            name: self.name.clone(),
            state: self.state,
            error: self.error.clone(),
            is_virtual: false,
        }
    }

    /// Records the outcome of an attempt to open the port.
    fn opened(&mut self, result: Result<MidiOutputConnection, String>) {
        match result {
            Ok(connection) => {
                log::info!("MIDI output opened: {}", self.name);
                self.connection = Some(connection);
                self.state = PortState::Connected;
                self.error.clear();
            }
            Err(error) => self.failed(error),
        }
    }

    fn failed(&mut self, error: String) {
        if self.state != PortState::Error || self.error != error {
            log::warn!("MIDI output {}: {error}", self.name);
        }
        self.connection = None;
        self.state = PortState::Error;
        self.error = error;
    }

    fn close(&mut self) {
        if self.connection.take().is_some() {
            log::info!("MIDI output closed: {}", self.name);
        }
        self.state = PortState::Disabled;
        self.error.clear();
    }
}

struct Poller {
    shared: Arc<Shared>,
    client_name: String,
    own_virtual_input: Option<String>,
    /// The client used to enumerate ports (kept for the lifetime of the thread).
    enumerator: Option<MidiOutput>,
    init_error_logged: bool,
    clients: Clients,
    entries: Vec<Entry>,
    /// Names of the ports something was sent to: kept open, reopened when they return.
    wanted: HashSet<String>,
    reported: Option<Vec<PortStatus>>,
    /// Report even if the list equals the last one: a port was replaced within one scan.
    force_report: bool,
}

impl Poller {
    fn new(shared: Arc<Shared>, options: OutputOptions) -> Self {
        let own_virtual_input = options
            .own_virtual_input
            .map(|name| name.trim().to_owned())
            .filter(|name| cfg!(unix) && !name.is_empty());
        Poller {
            shared,
            clients: Clients { name: options.client_name.clone(), idle: Vec::new() },
            client_name: options.client_name,
            own_virtual_input,
            enumerator: None,
            init_error_logged: false,
            entries: Vec::new(),
            wanted: HashSet::new(),
            reported: None,
            force_report: false,
        }
    }

    fn scan(&mut self, policy: &PortPolicy) {
        let Some(found) = self.enumerate() else {
            return;
        };
        let is_present =
            |entry: &Entry| found.iter().any(|f| f.raw_name == entry.raw_name && f.id == entry.id);
        // Vanished ports: dropping the entry closes its connection.
        let before = self.entries.len();
        self.entries.retain(|entry| {
            let present = is_present(entry);
            if !present {
                log::info!("MIDI output removed: {}", entry.name);
            }
            present
        });
        let removed = self.entries.len() != before;
        for entry in &mut self.entries {
            if let Some(f) = found.iter().find(|f| f.raw_name == entry.raw_name && f.id == entry.id)
            {
                entry.port = f.port.clone();
            }
        }

        // New ports, numbered deterministically (like the inputs).
        let mut new: Vec<&Found> = found
            .iter()
            .filter(|f| {
                !self.entries.iter().any(|e| e.raw_name == f.raw_name && e.id == f.id)
                    && !self.is_own_port(&f.base)
            })
            .collect();
        new.sort_by(|a, b| natural_cmp(&a.base, &b.base).then_with(|| natural_cmp(&a.id, &b.id)));
        new.dedup_by(|a, b| a.raw_name == b.raw_name && a.id == b.id);
        // Gone and back within one scan (for example moved to another USB port): the list may
        // read the same as before, but the device is a new one and has lost what it showed.
        self.force_report |= removed && !new.is_empty();
        for f in new {
            let name = self.unique_name(&f.base);
            log::info!("MIDI output found: {name}");
            self.entries.push(Entry {
                raw_name: f.raw_name.clone(),
                id: f.id.clone(),
                name,
                port: f.port.clone(),
                state: PortState::Disabled,
                error: String::new(),
                connection: None,
            });
        }

        // Close what the policy switches off; (re)open what is wanted and closed.
        for entry in &mut self.entries {
            if !policy.allows(&entry.name) {
                entry.close();
            } else if entry.connection.is_none() && self.wanted.contains(&entry.name) {
                let result = self.clients.open(&entry.port, &entry.name);
                entry.opened(result);
            }
        }
    }

    /// The ports present right now, or `None` when the MIDI system cannot be queried (in which
    /// case the known ports are left as they are).
    fn enumerate(&mut self) -> Option<Vec<Found>> {
        if self.enumerator.is_none() {
            match MidiOutput::new(&self.client_name) {
                Ok(output) => {
                    self.enumerator = Some(output);
                    self.init_error_logged = false;
                }
                Err(error) => {
                    if !self.init_error_logged {
                        log::warn!("MIDI output unavailable: {error}");
                        self.init_error_logged = true;
                    }
                    return None;
                }
            }
        }
        let output = self.enumerator.as_ref()?;
        let listed = catch_unwind(AssertUnwindSafe(|| {
            output
                .ports()
                .into_iter()
                .filter_map(|port| {
                    let raw_name = output.port_name(&port).ok()?; // vanished meanwhile
                    let id = port.id();
                    let base = normalize_port_name(&raw_name, &id);
                    Some(Found { id, raw_name, base, port })
                })
                .collect::<Vec<_>>()
        }));
        match listed {
            Ok(found) => Some(found),
            Err(_) => {
                log::error!("enumerating MIDI outputs failed; retrying with a new client");
                self.enumerator = None;
                None
            }
        }
    }

    /// Ports that are ours: the input side's virtual port, and on Linux every port of a client
    /// named like ours.
    fn is_own_port(&self, base: &str) -> bool {
        is_own_client_port(base, &self.client_name)
            || self.own_virtual_input.as_deref().is_some_and(|name| base == name)
    }

    /// `base`, or `base #2`, `base #3`, ... whichever is not taken yet.
    fn unique_name(&self, base: &str) -> String {
        let taken = |candidate: &str| self.entries.iter().any(|e| e.name == candidate);
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

    /// Sends one accepted send: opens the port first if nothing was sent to it yet.
    /// Marks the port called `name` as failed: its connection is dropped and the next poll opens
    /// it again while it is wanted.
    fn failed(&mut self, name: &str, error: &str) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.name == name) {
            entry.failed(error.to_owned());
        }
    }

    fn deliver(&mut self, pending: &Pending, policy: &PortPolicy) {
        if !policy.allows(&pending.port) {
            return; // switched off after `send()` checked
        }
        self.wanted.insert(pending.port.clone());
        let Some(entry) = self.entries.iter_mut().find(|e| e.name == pending.port) else {
            return; // vanished after `send()` checked
        };
        if entry.connection.is_none() {
            if entry.state == PortState::Error {
                return; // retried by the next poll, not by every send
            }
            let result = self.clients.open(&entry.port, &entry.name);
            entry.opened(result);
        }
        let Some(connection) = entry.connection.as_mut() else {
            return;
        };
        for message in &pending.messages {
            if let Err(error) = connection.send(message) {
                entry.failed(format!("Sending failed: {error}."));
                return;
            }
        }
    }

    /// Publishes the port list and emits `PortsChanged` if it differs from the last report (or a
    /// port was replaced, see `force_report`).
    fn report(&mut self) {
        let mut ports: Vec<PortStatus> = self.entries.iter().map(Entry::status).collect();
        ports.sort_by(|a, b| natural_cmp(&a.name, &b.name));
        if !self.force_report && self.reported.as_ref() == Some(&ports) {
            return;
        }
        self.force_report = false;
        *lock(&self.shared.ports) = ports.clone();
        self.reported = Some(ports.clone());
        self.shared.emit(OutputEvent::PortsChanged { ports });
    }

    fn close_all(&mut self) {
        self.entries.clear();
        self.enumerator = None;
        self.clients.idle.clear();
    }
}

/// OS MIDI clients for new connections: midir's `connect` consumes one client per open port.
/// Clients handed back by failed attempts are reused (on macOS a CoreMIDI client is never freed,
/// see the input side).
struct Clients {
    name: String,
    idle: Vec<MidiOutput>,
}

impl Clients {
    fn open(&mut self, port: &MidiOutputPort, name: &str) -> Result<MidiOutputConnection, String> {
        let output = match self.idle.pop() {
            Some(output) => output,
            None => MidiOutput::new(&self.name)
                .map_err(|e| format!("The MIDI system is not available ({e})."))?,
        };
        // midir names the connection's own port after this (visible in e.g. `aconnect -l`).
        let connection_name = name.replace('\0', "");
        match catch_unwind(AssertUnwindSafe(|| output.connect(port, &connection_name))) {
            Ok(Ok(connection)) => Ok(connection),
            Ok(Err(error)) => {
                let message = describe_connect_error(error.kind());
                self.idle.push(error.into_inner());
                Err(message)
            }
            Err(_) => Err("The MIDI driver failed unexpectedly while opening the port.".into()),
        }
    }
}

/// Turns a midir connect error into something the user can act on (see the input side: on
/// Windows a failing `midiOutOpen` practically always means another program holds the port).
fn describe_connect_error(kind: ConnectErrorKind) -> String {
    const WINMM_OPEN_FAILED: &str = "could not create Windows MM MIDI output port";
    match kind {
        ConnectErrorKind::InvalidPort => "The port disappeared while it was being opened.".into(),
        ConnectErrorKind::Other(message) if message.contains("MMSYSERR_ALLOCATED") => {
            format!("In use by another program. {SHARING_ADVICE}")
        }
        ConnectErrorKind::Other(message) if message.starts_with(WINMM_OPEN_FAILED) => {
            format!(
                "Could not open the port; it is probably in use by another program. {SHARING_ADVICE}"
            )
        }
        ConnectErrorKind::Other(message) => format!("Could not open the port: {message}."),
    }
}
