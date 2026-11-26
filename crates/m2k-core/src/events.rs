//! What the engine reports to the outside world (UI monitor, CLI). Shared contract; generates
//! TypeScript bindings.

use m2k_midi::{Derived, MidiMessage};
use m2k_output::Key;
use m2k_output::platform::ForegroundApp;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// A snapshot of the engine's live state. Emitted as [`EngineEvent::Status`] whenever it changes.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct EngineStatus {
    /// Output is paused by the user (tray / UI toggle / an `Engine` action).
    pub paused: bool,
    /// Output is suspended because the app's own window has focus
    /// (`settings.suspend_while_focused`).
    pub focus_suspended: bool,
    /// MIDI learn is in progress; actions are suppressed.
    pub learning: bool,
    /// The profile the user selected (resolved: never dangling unless there are no profiles).
    pub selected_profile: Option<String>,
    /// The profile whose mappings are live right now: the auto-switched one if an app rule
    /// matches, else the selected one.
    pub effective_profile: Option<String>,
    /// `true` when `effective_profile` was chosen by an app rule.
    pub auto_switched: bool,
    /// Names of the active layers, sorted.
    pub layers: Vec<String>,
    /// Keys currently held down by the engine.
    pub held_keys: Vec<Key>,
    /// Ids of mappings whose `Toggle` is currently latched on, or whose `Hold` is engaged.
    pub engaged_mappings: Vec<String>,
    /// The foreground application as last observed (only tracked while something needs it).
    pub foreground: Option<ForegroundApp>,
    /// Name of the output backend in use (`Injector::name`).
    pub backend: String,
}

/// Why a mapping matched but did not produce output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    Paused,
    /// The app's own window has focus.
    WindowFocused,
    Learning,
    RateLimited,
    ConditionFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Outcome {
    /// The action was sent to the OS.
    Sent,
    /// The mapping matched but nothing was sent.
    Skipped { reason: SkipReason },
    /// The OS / backend rejected it.
    Failed { error: String },
}

/// One received MIDI message, for the monitor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MidiLog {
    /// Microseconds since the engine started.
    #[ts(type = "number")]
    pub time_us: u64,
    pub device: String,
    pub message: MidiMessage,
    /// High-level events this message completed (NRPN/RPN data, 14-bit CC).
    pub derived: Vec<Derived>,
    /// Ids of the mappings whose source matched this message.
    pub mappings: Vec<String>,
}

/// One action a mapping fired (or tried to), for the monitor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct FiredLog {
    #[ts(type = "number")]
    pub time_us: u64,
    pub profile_id: String,
    pub mapping_id: String,
    /// Short human readable description of what was (or would have been) done, e.g.
    /// `"Ctrl+Shift+O"`, `"hold W"`, `"release W"`, `"type \"hello\""`, `"scroll up 1"`.
    pub description: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineEvent {
    Midi(MidiLog),
    Fired(FiredLog),
    Status(EngineStatus),
    /// Something the user should know about (backend failure, bad pattern in a mapping, ...).
    Error { message: String },
}
