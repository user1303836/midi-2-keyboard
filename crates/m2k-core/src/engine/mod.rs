//! The engine: turns MIDI messages into injected input according to a [`Config`].
//!
//! CONTRACT (the public API below is relied upon by the app runtime; keep it stable):
//!
//! - [`Engine::start`] spawns the engine thread and returns a cheap handle. Every method only
//!   posts a message to that thread and returns immediately; none of them block.
//! - The engine thread owns the [`Injector`], all runtime state and a timer queue. It never
//!   sleeps while handling an event: delays (tap duration, macros, repeats, rate limits,
//!   tap-hold / multi-tap decisions) are timers.
//! - Whatever happens (pause, config change, profile switch, device disconnect, shutdown, panic)
//!   keys and buttons held by the engine are released.
//!
//! # Inside
//!
//! - `worker`: the engine thread (loop, commands, MIDI dispatch, profiles, status, recovery).
//! - `compile`: config -> runnable form (actions as timed steps, parsed patterns, index).
//! - `source`: per-device MIDI state and the active/inactive/pulse vocabulary of sources.
//! - `behavior`: per-mapping instance tables and the behaviors (trigger, hold, toggle, ...).
//! - `limiter`: throttle / debounce.
//! - `cx`: the context behaviors act on (gating, layers, conditions, reporting).
//! - `executor`: action queues, hold engagements with key stagger, keyboard auto-repeat.
//! - `output`: reference-counted keys and buttons over the injector.
//! - `timers`, `pattern`, `text`, `foreground`, `launch`, `sys`: supporting pieces.

mod behavior;
mod compile;
mod cx;
mod executor;
mod foreground;
mod launch;
mod limiter;
mod output;
mod pattern;
mod source;
mod sys;
mod text;
mod timers;
mod worker;

use std::sync::{Arc, Mutex};
use std::thread::{JoinHandle, ThreadId};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use m2k_midi::MidiMessage;
use m2k_output::Injector;
use m2k_output::platform::ForegroundApp;

use crate::events::{EngineEvent, EngineStatus};
use crate::model::{Action, Config};
use worker::{Command, Envelope, Shared};

pub struct EngineOptions {
    /// The output backend. Use `m2k_output::NullInjector` for dry runs.
    pub injector: Box<dyn Injector>,
    /// Poll the foreground application (on a helper thread) while any profile or condition
    /// needs it. Disable for headless/CLI simulations.
    pub track_foreground: bool,
}

/// How long [`Engine::stop`] waits for the engine thread. Releasing everything takes
/// microseconds; the limit only matters if a backend call or the sink never returns, so that
/// the application can still exit.
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

/// Handle to the running engine.
pub struct Engine {
    tx: Sender<Envelope>,
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
    thread_id: ThreadId,
    /// Disconnects when the engine thread has finished (its sender lives on that thread).
    finished: Receiver<()>,
}

impl Engine {
    /// Starts the engine. `sink` receives monitor events and status snapshots; it is called from
    /// the engine thread and must be fast. It may be called before `start` returns.
    pub fn start(
        config: Config,
        options: EngineOptions,
        sink: impl Fn(EngineEvent) + Send + 'static,
    ) -> Engine {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (finished_tx, finished) = crossbeam_channel::bounded::<()>(0);
        let shared = Arc::new(Shared::default());
        let handle = {
            let tx = tx.clone();
            let shared = shared.clone();
            std::thread::Builder::new()
                .name("m2k-engine".into())
                .spawn(move || {
                    let _finished = finished_tx;
                    worker::run(config, options, Box::new(sink), shared, tx, rx);
                })
                .expect("could not start the engine thread")
        };
        let thread_id = handle.thread().id();
        Engine { tx, shared, thread: Mutex::new(Some(handle)), thread_id, finished }
    }

    fn post(&self, command: Command) {
        // After `stop` the engine thread is gone and this does nothing.
        let _ = self.tx.send(Envelope::now(command));
    }

    /// Hot-swaps the configuration. Runtime state of mappings whose definition did not change
    /// is preserved; everything held by removed/changed mappings is released.
    pub fn set_config(&self, config: Config) {
        self.post(Command::SetConfig(Box::new(config)));
    }

    /// Replaces the output backend (for example after the user changed injector settings).
    /// Everything held is released through the old backend first.
    pub fn set_injector(&self, injector: Box<dyn Injector>) {
        self.post(Command::SetInjector(injector));
    }

    /// Feeds one MIDI message received on `device` (normalized port name).
    /// `time_us` is the input layer's monotonic timestamp. The engine measures all timing
    /// (tap-hold, multi-tap windows, debounces) from the moment this is called, on its own
    /// clock, so `time_us` is informational only.
    pub fn midi(&self, device: &str, time_us: u64, message: MidiMessage) {
        let _ = time_us;
        self.post(Command::Midi { device: device.to_string(), message });
    }

    /// A device went away: forget its state and release whatever its notes/controls were holding.
    pub fn device_disconnected(&self, device: &str) {
        self.post(Command::DeviceDisconnected(device.to_string()));
    }

    pub fn set_paused(&self, paused: bool) {
        self.post(Command::SetPaused(paused));
    }

    /// Tells the engine whether the app's own window has keyboard focus.
    pub fn set_window_focused(&self, focused: bool) {
        self.post(Command::SetWindowFocused(focused));
    }

    /// While learning, mappings are still matched (and reported) but no action is executed.
    pub fn set_learning(&self, learning: bool) {
        self.post(Command::SetLearning(learning));
    }

    /// Selects a profile by id (same as changing `config.selected_profile`, without a reload).
    pub fn select_profile(&self, profile_id: &str) {
        self.post(Command::SelectProfile(profile_id.to_string()));
    }

    /// Overrides the observed foreground application (used when `track_foreground` is off).
    pub fn set_foreground(&self, app: Option<ForegroundApp>) {
        self.post(Command::SetForeground(app));
    }

    /// Runs an action after `delay_ms`, regardless of pause/focus suspension (the UI's "Test"
    /// button: "fires in 3 seconds, focus your target window").
    pub fn run_action(&self, action: Action, delay_ms: u32) {
        self.post(Command::RunAction { action: Box::new(action), delay_ms });
    }

    /// Releases everything the engine is holding and unlatches all toggles.
    pub fn release_all(&self) {
        self.post(Command::ReleaseAll);
    }

    /// The latest status snapshot.
    pub fn status(&self) -> EngineStatus {
        self.shared.status.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Releases everything and stops the engine thread, waiting for it to finish. Idempotent;
    /// after this every other method is a no-op. Also runs when the handle is dropped.
    ///
    /// Called on the engine thread itself (from the sink), it only requests the stop; the
    /// thread finishes right after the current event.
    pub fn stop(&self) {
        if std::thread::current().id() == self.thread_id {
            self.post(Command::Shutdown);
            return;
        }
        let mut thread = self.thread.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(handle) = thread.take() {
            self.post(Command::Shutdown);
            match self.finished.recv_timeout(STOP_TIMEOUT) {
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) | Ok(()) => {
                    let _ = handle.join();
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                    log::error!(
                        "the engine thread did not stop within {STOP_TIMEOUT:?}; leaving it behind"
                    );
                }
            }
        }
    }

    /// Same as [`Engine::stop`], consuming the handle.
    pub fn shutdown(self) {
        self.stop();
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}
