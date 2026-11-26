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

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::message::MidiMessage;

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
pub struct InputManager {}

impl InputManager {
    /// Starts the manager. `sink` is called from background threads.
    pub fn start(
        options: InputOptions,
        policy: PortPolicy,
        sink: impl Fn(InputEvent) + Send + Sync + 'static,
    ) -> Self {
        let _ = (options, policy, sink);
        todo!("m2k-midi: InputManager::start")
    }

    /// Replaces the policy; ports are opened/closed accordingly right away.
    pub fn set_policy(&self, policy: PortPolicy) {
        let _ = policy;
        todo!("m2k-midi: InputManager::set_policy")
    }

    /// Re-scans the system's ports immediately instead of waiting for the next poll.
    pub fn rescan(&self) {
        todo!("m2k-midi: InputManager::rescan")
    }

    /// Current list of present ports.
    pub fn ports(&self) -> Vec<PortStatus> {
        todo!("m2k-midi: InputManager::ports")
    }

    /// Delivers `bytes` through the sink as if they had been received on `port` (the port does
    /// not need to exist). Used by the UI's "simulate MIDI" tools and the CLI.
    pub fn inject(&self, port: &str, bytes: &[u8]) {
        let _ = (port, bytes);
        todo!("m2k-midi: InputManager::inject")
    }
}
