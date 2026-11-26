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

use m2k_midi::MidiMessage;
use m2k_output::Injector;
use m2k_output::platform::ForegroundApp;

use crate::events::{EngineEvent, EngineStatus};
use crate::model::{Action, Config};

pub struct EngineOptions {
    /// The output backend. Use `m2k_output::NullInjector` for dry runs.
    pub injector: Box<dyn Injector>,
    /// Poll the foreground application (on a helper thread) while any profile or condition
    /// needs it. Disable for headless/CLI simulations.
    pub track_foreground: bool,
}

/// Handle to the running engine.
pub struct Engine {}

impl Engine {
    /// Starts the engine. `sink` receives monitor events and status snapshots; it is called from
    /// the engine thread and must be fast.
    pub fn start(
        config: Config,
        options: EngineOptions,
        sink: impl Fn(EngineEvent) + Send + 'static,
    ) -> Engine {
        let _ = (config, options, sink);
        todo!("m2k-core: Engine::start")
    }

    /// Hot-swaps the configuration. Runtime state of mappings whose definition did not change
    /// is preserved; everything held by removed/changed mappings is released.
    pub fn set_config(&self, config: Config) {
        let _ = config;
        todo!("m2k-core: Engine::set_config")
    }

    /// Replaces the output backend (for example after the user changed injector settings).
    /// Everything held is released through the old backend first.
    pub fn set_injector(&self, injector: Box<dyn Injector>) {
        let _ = injector;
        todo!("m2k-core: Engine::set_injector")
    }

    /// Feeds one MIDI message received on `device` (normalized port name).
    /// `time_us` is the input layer's monotonic timestamp.
    pub fn midi(&self, device: &str, time_us: u64, message: MidiMessage) {
        let _ = (device, time_us, message);
        todo!("m2k-core: Engine::midi")
    }

    /// A device went away: forget its state and release whatever its notes/controls were holding.
    pub fn device_disconnected(&self, device: &str) {
        let _ = device;
        todo!("m2k-core: Engine::device_disconnected")
    }

    pub fn set_paused(&self, paused: bool) {
        let _ = paused;
        todo!("m2k-core: Engine::set_paused")
    }

    /// Tells the engine whether the app's own window has keyboard focus.
    pub fn set_window_focused(&self, focused: bool) {
        let _ = focused;
        todo!("m2k-core: Engine::set_window_focused")
    }

    /// While learning, mappings are still matched (and reported) but no action is executed.
    pub fn set_learning(&self, learning: bool) {
        let _ = learning;
        todo!("m2k-core: Engine::set_learning")
    }

    /// Selects a profile by id (same as changing `config.selected_profile`, without a reload).
    pub fn select_profile(&self, profile_id: &str) {
        let _ = profile_id;
        todo!("m2k-core: Engine::select_profile")
    }

    /// Overrides the observed foreground application (used when `track_foreground` is off).
    pub fn set_foreground(&self, app: Option<ForegroundApp>) {
        let _ = app;
        todo!("m2k-core: Engine::set_foreground")
    }

    /// Runs an action after `delay_ms`, regardless of pause/focus suspension (the UI's "Test"
    /// button: "fires in 3 seconds, focus your target window").
    pub fn run_action(&self, action: Action, delay_ms: u32) {
        let _ = (action, delay_ms);
        todo!("m2k-core: Engine::run_action")
    }

    /// Releases everything the engine is holding and unlatches all toggles.
    pub fn release_all(&self) {
        todo!("m2k-core: Engine::release_all")
    }

    /// The latest status snapshot.
    pub fn status(&self) -> EngineStatus {
        todo!("m2k-core: Engine::status")
    }

    /// Releases everything and stops the engine thread, waiting for it to finish.
    pub fn shutdown(self) {
        todo!("m2k-core: Engine::shutdown")
    }
}
