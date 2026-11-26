//! Types exchanged between the application runtime (engine + MIDI input + config store) and a
//! front end. Shared contract; generates TypeScript bindings.

use m2k_midi::input::PortStatus;
use m2k_output::Key;
use m2k_output::platform::Permissions;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::events::{EngineEvent, EngineStatus};
use crate::model::Config;

/// Static facts about the running application.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
pub struct AppInfo {
    pub version: String,
    /// `"windows"`, `"macos"` or `"linux"`.
    pub platform: String,
    /// Absolute path of the config file.
    pub config_path: String,
    /// `true` when the display server cannot tell us about the foreground app or accept
    /// injected input without extra setup (Wayland).
    pub wayland: bool,
    /// The app runs with administrator rights (Windows), which it needs to control other
    /// programs that do.
    #[serde(default)]
    pub elevated: bool,
}

/// Everything a front end needs to render its first frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Snapshot {
    pub info: AppInfo,
    pub config: Config,
    pub engine: EngineStatus,
    pub ports: Vec<PortStatus>,
    /// MIDI output ports (for "Send MIDI" actions and feedback). `Connected` = open (they are
    /// opened on first use), `Disabled` = present but not in use, `Error` = could not be opened.
    #[serde(default)]
    pub outputs: Vec<PortStatus>,
    pub permissions: Permissions,
    /// Problems found while loading the config (recovered corruption, repairs, ...).
    pub warnings: Vec<String>,
    /// Why the output backend could not be created; empty when it is working.
    pub backend_error: String,
    /// Keys the output backend of this platform/session cannot send.
    #[serde(default)]
    pub unsupported_keys: Vec<Key>,
}

/// Live updates pushed to the front end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    /// A batch of engine events, oldest first.
    Engine { events: Vec<EngineEvent> },
    /// The set of MIDI ports or their states changed.
    Ports { ports: Vec<PortStatus> },
    /// The set of MIDI output ports or their states changed.
    Outputs { outputs: Vec<PortStatus> },
    Permissions {
        permissions: Permissions,
        backend_error: String,
        #[serde(default)]
        unsupported_keys: Vec<Key>,
    },
    /// The config was changed by something other than this front end (an action switched the
    /// selected profile, the file was re-imported, ...).
    Config { config: Config },
}
