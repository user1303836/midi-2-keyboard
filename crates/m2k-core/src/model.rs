//! The configuration model: everything the user can set up, exactly as persisted to JSON and
//! exchanged with the UI. TypeScript bindings are generated from these types (`npm run bindings`).
//!
//! # Vocabulary
//!
//! - A **profile** is a named set of mappings. One profile is *selected*; profiles marked
//!   `always_on` are active in addition to it. A profile can auto-activate for certain apps.
//! - A **mapping** connects a **source** (which MIDI messages) through a **behavior** (how that
//!   stream of messages is interpreted as a gesture) to **actions** (what is sent to the OS).
//!   **Conditions** gate it, a **rate limit** tames it.
//!
//! # Sources are *active* or *inactive*, and carry a *value*
//!
//! Every source reduces to the same two signals, which is what lets any behavior work with any
//! source:
//!
//! | Source                         | Active while                                   | Value            |
//! |--------------------------------|------------------------------------------------|------------------|
//! | Note                           | note on (velocity in range) until its note off | velocity         |
//! | Chord                          | all of its notes are down                      | last velocity    |
//! | CC / CC14 / NRPN / RPN         | last value is inside `range`                   | the value        |
//! | Pitch bend / pressure          | last value is inside `range`                   | the value        |
//! | Program change, song select    | a pulse (active then immediately inactive)     | program / song   |
//! | SysEx, clock, start/stop/...   | a pulse                                        | 0                |
//!
//! A source that matches several notes/channels/devices tracks each of them separately; the
//! mapping as a whole is active while at least one of them is.
//!
//! # Writing a config by hand
//!
//! Everything with an obvious value has a default, so a hand-written file only needs what makes
//! a mapping what it is (the event type and its note / controller / number / pattern, the
//! behavior type, the keys). A range can be written as a single number (`"notes": 60`) or with
//! one end left out (`{"min": 64}` = 64 and up). A ranged source written without `range` gets
//! the range its behavior needs (see `store::parse`).
//!
//! # Compatibility
//!
//! Persisted in user config files. Only ever add fields (with `#[serde(default)]`) or enum
//! variants; never rename or remove without writing a migration in `store.rs`.

use std::fmt;

use m2k_output::{InjectorOptions, Key, MouseButton};
use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Current config schema version.
pub const CONFIG_VERSION: u32 = 1;

fn yes() -> bool {
    true
}

fn default_version() -> u32 {
    CONFIG_VERSION
}

// ---------------------------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------------------------

/// The whole persisted configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Config {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub settings: Settings,
    /// Devices the app has seen or the user has configured.
    #[serde(default)]
    pub devices: Vec<DeviceConfig>,
    #[serde(default)]
    pub profiles: Vec<Profile>,
    /// Id of the profile the user selected. `None` (or a dangling id) means the first profile.
    #[serde(default)]
    pub selected_profile: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: CONFIG_VERSION,
            settings: Settings::default(),
            devices: Vec::new(),
            profiles: vec![Profile {
                id: "default".into(),
                name: "Default".into(),
                always_on: false,
                apps: Vec::new(),
                mappings: Vec::new(),
            }],
            selected_profile: Some("default".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Dark,
    Light,
}

/// Global settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Settings {
    #[serde(default)]
    pub theme: Theme,
    /// Octave number displayed for MIDI note 60: `4` (scientific, "C4") or `3` (Yamaha, "C3").
    #[serde(default = "default_middle_c_octave")]
    pub middle_c_octave: i8,
    /// Start hidden in the tray.
    #[serde(default)]
    pub start_minimized: bool,
    /// Closing the window keeps the app running in the tray.
    #[serde(default = "yes")]
    pub close_to_tray: bool,
    #[serde(default)]
    pub launch_at_login: bool,
    /// Do not send any output while the midi-2-keyboard window itself has keyboard focus
    /// (prevents a mapping from typing into / closing the editor). The monitor still shows what
    /// would have been sent.
    #[serde(default = "yes")]
    pub suspend_while_focused: bool,
    /// Follow the foreground application: activate profiles whose `apps` match.
    #[serde(default = "yes")]
    pub auto_switch_profiles: bool,
    /// Open newly discovered MIDI devices automatically.
    #[serde(default = "yes")]
    pub connect_new_devices: bool,
    /// How long a tapped key stays down, in milliseconds, unless the action overrides it.
    /// Some games ignore taps shorter than a frame.
    #[serde(default = "default_tap_ms")]
    pub tap_ms: u32,
    /// Delay between consecutive key events inside one combination (modifier down -> key down),
    /// in milliseconds. 0 sends them back to back.
    #[serde(default)]
    pub key_stagger_ms: u32,
    /// Emulated keyboard auto-repeat for `Hold` behaviors: delay before the first repeat.
    #[serde(default = "default_repeat_delay_ms")]
    pub repeat_delay_ms: u32,
    /// Emulated keyboard auto-repeat for `Hold` behaviors: interval between repeats.
    #[serde(default = "default_repeat_interval_ms")]
    pub repeat_interval_ms: u32,
    /// Output backend selection / tuning.
    #[serde(default)]
    pub injector: InjectorOptions,
    /// System-wide shortcut that pauses / resumes all output, also while another application
    /// has focus: a safety valve when a mapping misbehaves. Modifier keys plus exactly one other
    /// key, e.g. `["ControlLeft", "AltLeft", "Pause"]`; left and right modifiers count the same.
    /// Empty: no shortcut. While registered, the OS hands the combination to midi-2-keyboard
    /// instead of the focused application.
    #[serde(default)]
    pub pause_hotkey: Vec<Key>,
}

fn default_middle_c_octave() -> i8 {
    4
}
fn default_tap_ms() -> u32 {
    25
}
fn default_repeat_delay_ms() -> u32 {
    400
}
fn default_repeat_interval_ms() -> u32 {
    33
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: Theme::default(),
            middle_c_octave: default_middle_c_octave(),
            start_minimized: false,
            close_to_tray: true,
            launch_at_login: false,
            suspend_while_focused: true,
            auto_switch_profiles: true,
            connect_new_devices: true,
            tap_ms: default_tap_ms(),
            key_stagger_ms: 0,
            repeat_delay_ms: default_repeat_delay_ms(),
            repeat_interval_ms: default_repeat_interval_ms(),
            injector: InjectorOptions::default(),
            pause_hotkey: Vec::new(),
        }
    }
}

/// A MIDI input device the app knows about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DeviceConfig {
    /// Port name as reported by `m2k-midi` (already normalized to be stable across reconnects).
    pub name: String,
    /// Friendly name shown in the UI. Empty = use `name`.
    #[serde(default)]
    pub alias: String,
    /// Disabled devices are never opened.
    #[serde(default = "yes")]
    pub enabled: bool,
}

// ---------------------------------------------------------------------------------------------
// Profiles
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Profile {
    /// Empty (or missing) = one is assigned when the config is loaded.
    #[serde(default)]
    pub id: String,
    /// Empty (or missing) = "Untitled".
    #[serde(default)]
    pub name: String,
    /// Mappings of an always-on profile are active no matter which profile is selected
    /// (use it for global controls such as "switch profile" pads).
    #[serde(default)]
    pub always_on: bool,
    /// Auto-activate this profile while one of these applications has focus
    /// (when `settings.auto_switch_profiles` is on).
    #[serde(default)]
    pub apps: Vec<AppMatcher>,
    #[serde(default)]
    pub mappings: Vec<Mapping>,
}

/// Matches the foreground application. Both patterns are case-insensitive and support `*`
/// (any run of characters) and `?` (any single character); a pattern without wildcards matches
/// as a substring. An empty pattern matches anything. Both must match.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
pub struct AppMatcher {
    /// Matched against the process / executable name (and the platform app id).
    #[serde(default)]
    pub process: String,
    /// Matched against the focused window title.
    #[serde(default)]
    pub title: String,
}

// ---------------------------------------------------------------------------------------------
// Mappings
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Mapping {
    /// Empty (or missing) = one is assigned when the config is loaded.
    #[serde(default)]
    pub id: String,
    /// Optional user label. Empty = the UI shows an auto-generated summary.
    #[serde(default)]
    pub name: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    pub source: Source,
    pub behavior: Behavior,
    /// All conditions must hold at the moment the mapping would fire.
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// Limits how often this mapping's actions fire. Applies to `Trigger` (press/release),
    /// `Cycle`, `Change` and `Step`; ignored by `Hold`, `Toggle`, `TapHold` and `MultiTap`.
    #[serde(default)]
    pub rate_limit: Option<RateLimit>,
    /// MIDI sent back to the controller to show a state (light a pad while a toggle is on, ...).
    #[serde(default)]
    pub feedback: Option<Feedback>,
}

/// MIDI sent to a controller whenever a state turns on or off, so its lights (or motor faders,
/// or displays) show what the computer side is doing.
///
/// `on` is sent when the state described by `when` becomes true, `off` when it becomes false.
/// Both are also sent to bring a controller up to date: when the output port (re)appears, when
/// the configuration changes, and when the set of live profiles changes (a mapping that stops
/// being live gets its `off`). On exit and on pause nothing is sent: lights keep showing the
/// state. Sending never blocks and never delays key output; a missing port is reported once, not
/// per message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Feedback {
    /// Output port to send to. `None` = the output named like the mapping's source device; if
    /// the source accepts any device, the one named like the device that last reached this
    /// mapping (so nothing is sent before the control was touched once).
    #[serde(default)]
    pub device: Option<String>,
    #[serde(default)]
    pub when: FeedbackWhen,
    pub on: MidiSend,
    pub off: MidiSend,
}

/// The state a [`Feedback`] shows.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FeedbackWhen {
    /// While this mapping is engaged — what the UI shows as ON: a `Hold` / `TapHold` target
    /// that is down, a latched `Toggle`.
    #[default]
    Engaged,
    /// While the named layer is on, whoever switched it (a "shift" light).
    Layer { layer: String },
    /// While the profile with this id is the effective one (radio lights on profile pads).
    Profile { id: String },
}

/// A MIDI message to send. Channels are 1..=16.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MidiSend {
    /// Note on. Velocity 0 is how most pads are told to go dark.
    NoteOn { channel: u8, note: u8, velocity: SendValue },
    NoteOff { channel: u8, note: u8 },
    ControlChange { channel: u8, controller: u8, value: SendValue },
    ProgramChange { channel: u8, program: SendValue },
    /// Any bytes as space separated hex (`F0 7F 7F 06 02 F7`): SysEx, several messages in a
    /// row, anything the other variants do not cover. Sent exactly as written.
    Raw { bytes: String },
}

impl MidiSend {
    /// Nothing to send (raw bytes left blank).
    pub fn is_empty(&self) -> bool {
        matches!(self, MidiSend::Raw { bytes } if bytes.trim().is_empty())
    }
}

/// Where a data byte of a [`MidiSend`] comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SendValue {
    Fixed { value: u8 },
    /// The value of the message that triggered the action, as 0..=127 (a note's velocity, a
    /// controller's value, a program number; 14-bit values give their upper 7 bits). In a
    /// [`Feedback`], where no message is at hand, it stands for 127 in `on` and 0 in `off`.
    Incoming,
}

/// An inclusive range. Used for 7-bit values (0..=127), 14-bit values (0..=16383), note numbers
/// and program numbers alike.
///
/// Always written as `{"min": .., "max": ..}`. When reading, a single number means just that
/// value (`60`), and either end may be left out (`{"min": 64}` = 64 and up, `{"max": 63}` = up
/// to 63; a missing `max` reads as the largest value, which loading clamps to the type's limit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
pub struct ValueRange {
    pub min: u16,
    pub max: u16,
}

impl<'de> Deserialize<'de> for ValueRange {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(RangeVisitor)
    }
}

/// Reads a [`ValueRange`] from a number or a `{min, max}` object with optional ends.
struct RangeVisitor;

impl<'de> Visitor<'de> for RangeVisitor {
    type Value = ValueRange;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(
            "a number such as 60, or a range such as {\"min\": 60, \"max\": 72} (either end may \
             be left out)",
        )
    }

    fn visit_u64<E: de::Error>(self, value: u64) -> Result<ValueRange, E> {
        match u16::try_from(value) {
            Ok(value) => Ok(ValueRange::one(value)),
            Err(_) => Err(E::invalid_value(de::Unexpected::Unsigned(value), &self)),
        }
    }

    fn visit_i64<E: de::Error>(self, value: i64) -> Result<ValueRange, E> {
        match u64::try_from(value) {
            Ok(value) => self.visit_u64(value),
            Err(_) => Err(E::invalid_value(de::Unexpected::Signed(value), &self)),
        }
    }

    fn visit_f64<E: de::Error>(self, value: f64) -> Result<ValueRange, E> {
        // `60.0` is a whole number too.
        if value.fract() == 0.0 && (0.0..=f64::from(u16::MAX)).contains(&value) {
            return Ok(ValueRange::one(value as u16));
        }
        Err(E::invalid_value(de::Unexpected::Float(value), &self))
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ValueRange, A::Error> {
        let (mut min, mut max) = (None, None);
        while let Some(key) = map.next_key::<String>()? {
            let slot = match key.as_str() {
                "min" => &mut min,
                "max" => &mut max,
                other => return Err(de::Error::unknown_field(other, &["min", "max"])),
            };
            if slot.is_some() {
                return Err(de::Error::custom(format_args!("`{key}` is given twice")));
            }
            *slot = Some(map.next_value::<u16>()?);
        }
        Ok(ValueRange { min: min.unwrap_or(0), max: max.unwrap_or(u16::MAX) })
    }
}

fn all_7bit() -> ValueRange {
    ValueRange::new(0, 127)
}
fn all_14bit() -> ValueRange {
    ValueRange::new(0, 16_383)
}
fn all_velocities() -> ValueRange {
    ValueRange::new(1, 127)
}
/// The upper half of a 7-bit control: a switch or pedal that is "on".
fn on_7bit() -> ValueRange {
    ValueRange::new(64, 127)
}

impl ValueRange {
    pub const fn new(min: u16, max: u16) -> Self {
        ValueRange { min, max }
    }
    pub const fn one(value: u16) -> Self {
        ValueRange { min: value, max: value }
    }
    pub fn contains(&self, value: u16) -> bool {
        value >= self.min && value <= self.max
    }
}

/// Which MIDI messages feed a mapping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Source {
    /// Only this input device (port name as in [`DeviceConfig::name`]). `None` = any device.
    #[serde(default)]
    pub device: Option<String>,
    /// Only this MIDI channel, 1..=16. `None` = any channel. Ignored for system messages.
    #[serde(default)]
    pub channel: Option<u8>,
    pub event: SourceEvent,
    /// Extra distance the value must travel *beyond* the edge of the event's `range` before an
    /// active source counts as inactive again (it still becomes active exactly at the edge).
    /// Stops a jittery pedal, fader or pressure sensor from flickering around the threshold.
    /// Applies to ranged, absolute sources (CC, 14-bit CC, NRPN/RPN, pitch bend, pressure, song
    /// position); ignored by everything else. `0` = off.
    #[serde(default)]
    pub hysteresis: u16,
    /// Contact-bounce filter for switches, pedals and worn pads, in milliseconds. A press or
    /// release is taken immediately (no added latency); further press/release changes of the
    /// same source instance are then ignored for this long, and when that time is over the
    /// source is brought in line with the control's actual state (so a real, quick release is
    /// delayed, never lost). Applies to every behavior. `0` = off.
    #[serde(default)]
    pub debounce_ms: u32,
}

/// How a controller encodes its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ValueEncoding {
    /// A position: knobs with end stops, faders, pedals, buttons.
    #[default]
    Absolute,
    /// Endless encoder, two's complement: 1..=63 = +n, 127..=65 = -(128 - v). (0 and 64 = no move.)
    RelativeTwosComplement,
    /// Endless encoder, offset around 64: 65..=127 = +(v - 64), 63..=0 = -(64 - v).
    RelativeBinaryOffset,
    /// Endless encoder, sign-magnitude: 1..=63 = +n, 65..=127 = -(v - 64).
    RelativeSignMagnitude,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SourceEvent {
    /// Note on/off. `notes` selects note numbers (a single note has `min == max`),
    /// `velocity` filters note-on velocity (1..=127).
    Note {
        notes: ValueRange,
        #[serde(default = "all_velocities")]
        velocity: ValueRange,
    },
    /// Several notes held at the same time. Tracked per device and channel: the source is active
    /// while **all** of `notes` are down — it presses when the last of them goes down and
    /// releases as soon as any of them goes up (it can press again once that note returns).
    /// With `window_ms > 0` the notes must have been struck within that many milliseconds of
    /// each other, so playing them one after another does not count; `0` = no time limit
    /// (holding some and adding the rest later works). Other notes being held do not matter.
    /// The value is the velocity of the last note struck. Mappings on the individual notes are
    /// not suppressed.
    Chord {
        notes: Vec<u8>,
        #[serde(default)]
        window_ms: u32,
    },
    /// 7-bit control change. With an absolute `encoding` the source is active while the value is
    /// inside `range`. With a relative encoding every message is a pulse whose value is a signed
    /// tick count (`range` is ignored).
    ///
    /// Press-only buttons: many foot switches and cheap pads send their "on" value on every
    /// press and never a release. A real knob or fader never sends the same value twice in a
    /// row, so for every absolute ranged source (CC, 14-bit CC, NRPN/RPN data, pressure, pitch
    /// bend) an in-range message that **repeats the previous value** while the source is active
    /// counts as a fresh press (release + press for `Trigger`, `Toggle`, `Cycle`, `TapHold` and
    /// `MultiTap`; `Hold` stays held).
    ControlChange {
        controller: u8,
        #[serde(default = "all_7bit")]
        range: ValueRange,
        #[serde(default)]
        encoding: ValueEncoding,
    },
    /// 14-bit control change: MSB `controller` (0..=31) paired with LSB `controller + 32`.
    /// `range` is 0..=16383.
    ControlChange14 {
        controller: u8,
        #[serde(default = "all_14bit")]
        range: ValueRange,
    },
    /// Non-registered parameter. `hires` = 14-bit data (fires on data entry LSB, range
    /// 0..=16383); otherwise 7-bit (fires on data entry MSB, range 0..=127). Data
    /// increment/decrement messages always count as relative ticks (+1 / -1).
    Nrpn {
        number: u16,
        #[serde(default = "all_14bit")]
        range: ValueRange,
        #[serde(default)]
        hires: bool,
    },
    /// Registered parameter. Same semantics as [`SourceEvent::Nrpn`].
    Rpn {
        number: u16,
        #[serde(default = "all_14bit")]
        range: ValueRange,
        #[serde(default)]
        hires: bool,
    },
    /// Program change (a pulse). `programs` selects program numbers 0..=127.
    ProgramChange {
        #[serde(default = "all_7bit")]
        programs: ValueRange,
    },
    /// Pitch bend, `range` within 0..=16383 (8192 = center).
    PitchBend {
        #[serde(default = "all_14bit")]
        range: ValueRange,
    },
    /// Channel pressure (mono aftertouch).
    ChannelPressure {
        #[serde(default = "all_7bit")]
        range: ValueRange,
    },
    /// Polyphonic key pressure for the selected notes.
    PolyPressure {
        #[serde(default = "all_7bit")]
        notes: ValueRange,
        #[serde(default = "all_7bit")]
        range: ValueRange,
    },
    /// System exclusive message matching `pattern` (a pulse).
    ///
    /// The pattern is space separated hex bytes; `??` matches any one byte, `**` matches any run
    /// of bytes (including none). Example: `F0 7F ?? 06 02 F7` (MMC Play from any device id).
    /// The leading `F0` / trailing `F7` are optional in the pattern.
    SysEx { pattern: String },
    /// MIDI clock (a pulse every `divisor` ticks; 24 ticks = one quarter note). The tick counter
    /// restarts on Start.
    Clock {
        #[serde(default = "default_divisor")]
        divisor: u16,
    },
    /// Song select (a pulse).
    SongSelect {
        #[serde(default = "all_7bit")]
        songs: ValueRange,
    },
    /// Song position pointer, `range` within 0..=16383.
    SongPosition {
        #[serde(default = "all_14bit")]
        range: ValueRange,
    },
    /// Other system messages (pulses).
    System { message: SystemMessage },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum SystemMessage {
    Start,
    Continue,
    Stop,
    TuneRequest,
    Reset,
    ActiveSensing,
}

// ---------------------------------------------------------------------------------------------
// Behaviors
// ---------------------------------------------------------------------------------------------

/// How the stream of matching messages is interpreted, and what it does.
///
/// "Press" below means the source becoming active, "release" means it becoming inactive
/// (see the module docs). Pulse sources press and release in the same instant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Behavior {
    /// Fire an action on press and/or on release. With `repeat`, `on_press` fires again and
    /// again while the source stays active (turbo / auto-fire).
    Trigger {
        #[serde(default)]
        on_press: Option<Action>,
        #[serde(default)]
        on_release: Option<Action>,
        #[serde(default)]
        repeat: Option<Repeat>,
    },
    /// Hold the target for as long as the source is active: keys go down on press and up on
    /// release, exactly like a physical key. With `auto_repeat` the engine emulates the keyboard's
    /// typematic repeat (using `settings.repeat_*`) on platforms that do not repeat injected keys
    /// by themselves.
    Hold {
        target: HoldTarget,
        #[serde(default = "yes")]
        auto_repeat: bool,
    },
    /// Latch: the first press engages the target, the next press releases it.
    Toggle { target: HoldTarget },
    /// Each press fires the next action in the list, wrapping around. With `reset_after_ms`,
    /// the cycle restarts from the first action after that much inactivity.
    Cycle {
        actions: Vec<Action>,
        #[serde(default)]
        reset_after_ms: Option<u32>,
    },
    /// Tap versus hold: releasing before `hold_ms` fires `tap`. Staying active for `hold_ms`
    /// fires `hold` (once, at that moment) and engages `hold_target` until release.
    /// (`tap: Esc`, `hold_target: Ctrl` gives a dual-role key.)
    TapHold {
        #[serde(default)]
        tap: Option<Action>,
        #[serde(default)]
        hold: Option<Action>,
        #[serde(default)]
        hold_target: Option<HoldTarget>,
        #[serde(default = "default_tap_hold_ms")]
        hold_ms: u32,
    },
    /// Single / double / triple press, and optionally a long press, on one control.
    ///
    /// Without `hold`: the decision is made `window_ms` after the last press (or immediately
    /// once the highest configured count is reached).
    ///
    /// With `hold`: a press only counts as a tap when it is released before `hold_ms`; the
    /// decision is made `window_ms` after the last such release (or immediately at the highest
    /// configured count). A press that stays down for `hold_ms` fires `hold` once, right then,
    /// and discards any taps counted so far — so a long press never also fires the single-press
    /// action.
    MultiTap {
        #[serde(default)]
        single: Option<Action>,
        #[serde(default)]
        double: Option<Action>,
        #[serde(default)]
        triple: Option<Action>,
        #[serde(default = "default_window_ms")]
        window_ms: u32,
        #[serde(default)]
        hold: Option<Action>,
        #[serde(default = "default_hold_ms")]
        hold_ms: u32,
    },
    /// Fire on every matching message (while the value is inside the source range). This is the
    /// natural behavior for "when I move this knob, do X"; combine with a rate limit.
    ///
    /// `min_change` guards against a worn or noisy control that flutters by a step or two while
    /// nobody touches it: a message only counts when its value is at least that far away from
    /// the value of the last message that counted (the first message after start-up or
    /// reconnecting always counts). The reference moves whenever a message counts, whether or
    /// not the rate limit then lets the action through. Absolute sources only; relative
    /// encoders and pulses are never filtered. `0` and `1` = every message counts.
    Change {
        action: Action,
        #[serde(default)]
        min_change: u16,
    },
    /// Fire `up` when the value increases and `down` when it decreases: one action per `step`
    /// units of movement (absolute controls) or per `step` ticks (relative encoders,
    /// NRPN/RPN increment/decrement). The first message after (re)connecting only sets the
    /// reference position. A single message never fires more than `max_burst` actions
    /// (0 = unlimited).
    Step {
        #[serde(default)]
        up: Option<Action>,
        #[serde(default)]
        down: Option<Action>,
        #[serde(default = "default_step")]
        step: u16,
        #[serde(default = "default_max_burst")]
        max_burst: u16,
        /// Swap the directions.
        #[serde(default)]
        invert: bool,
        /// A second speed for fast movement (coarse steps for a quick spin, fine steps for a
        /// slow one).
        #[serde(default)]
        fast: Option<FastStep>,
    },
}

/// The "turned fast" half of [`Behavior::Step`]: when a single message moves the control by at
/// least `ticks` steps at once (an accelerating encoder spun quickly, a knob swept quickly),
/// the fast action of that direction fires **once**, instead of the normal action once per
/// step. A direction without a fast action keeps its normal behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct FastStep {
    /// How many steps in one message count as fast (at least 2).
    #[serde(default = "default_fast_ticks")]
    pub ticks: u16,
    #[serde(default)]
    pub up: Option<Action>,
    #[serde(default)]
    pub down: Option<Action>,
}

fn default_step() -> u16 {
    1
}
fn default_hold_ms() -> u32 {
    500
}
fn default_tap_hold_ms() -> u32 {
    350
}
fn default_window_ms() -> u32 {
    300
}
fn default_fast_ticks() -> u16 {
    3
}
fn default_divisor() -> u16 {
    24
}
fn default_repeat_delay() -> u32 {
    400
}
fn default_repeat_interval() -> u32 {
    100
}
fn default_max_burst() -> u16 {
    8
}

/// Auto-fire timing for [`Behavior::Trigger`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Repeat {
    /// Time between the press and the first repeat.
    #[serde(default = "default_repeat_delay")]
    pub delay_ms: u32,
    /// Time between repeats.
    #[serde(default = "default_repeat_interval")]
    pub interval_ms: u32,
}

/// Something that can be held: engaged on press, disengaged on release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HoldTarget {
    /// Keys go down in order and come up in reverse order.
    Keys { keys: Vec<Key> },
    MouseButton { button: MouseButton },
    /// A layer is active while held (a "shift" button for other mappings' conditions).
    Layer { layer: String },
}

impl HoldTarget {
    /// Nothing to hold (no keys, or a layer left blank): the engine treats it as no target.
    pub fn is_empty(&self) -> bool {
        match self {
            HoldTarget::Keys { keys } => keys.is_empty(),
            HoldTarget::MouseButton { .. } => false,
            HoldTarget::Layer { layer } => layer.trim().is_empty(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------------------------

/// Something that happens at one instant (or plays out over time, for sequences).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Tap a key combination: keys go down in order, stay down for `tap_ms`
    /// (default `settings.tap_ms`), then come up in reverse order.
    Keys {
        keys: Vec<Key>,
        #[serde(default)]
        tap_ms: Option<u32>,
    },
    /// Type Unicode text. `char_delay_ms` > 0 types one character at a time with that pause.
    /// The placeholders `{value}`, `{note}`, `{note_name}`, `{velocity}`, `{channel}` and
    /// `{controller}` are replaced with data from the triggering message.
    Text {
        text: String,
        #[serde(default)]
        char_delay_ms: u32,
    },
    MouseClick {
        button: MouseButton,
        #[serde(default = "default_clicks")]
        clicks: u8,
    },
    /// Scroll by wheel notches (positive `dy` = up, positive `dx` = right).
    MouseScroll {
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    /// Move the pointer, relative (pixels) or to an absolute position.
    MouseMove {
        #[serde(default)]
        x: i32,
        #[serde(default)]
        y: i32,
        #[serde(default)]
        absolute: bool,
    },
    /// A macro: steps run in order; `Delay` steps wait without blocking anything else.
    Sequence { steps: Vec<SequenceStep> },
    SwitchProfile { target: ProfileTarget },
    /// Turn a named layer on/off. Layers are referenced by [`Condition::Layer`].
    Layer {
        layer: String,
        #[serde(default = "default_layer_op")]
        op: LayerOp,
    },
    Engine { op: EngineOp },
    /// Launch a program. `command` is the executable (looked up on `PATH`), `args` are passed
    /// verbatim (no shell). The process is detached and never waited for.
    Run {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        working_dir: String,
    },
    /// Open a file, folder or URL with the default application.
    Open { target: String },
    /// Send a MIDI message to an output port: light a pad, move a motor fader, forward or
    /// translate a control to other gear or software.
    Midi {
        /// Output port to send to (a name from `Snapshot::outputs`). `None` = the output named
        /// like the device the triggering message came from, i.e. back to the controller
        /// itself. When there is no such output the firing is reported as failed.
        #[serde(default)]
        device: Option<String>,
        message: MidiSend,
    },
}

fn default_clicks() -> u8 {
    1
}
fn default_layer_op() -> LayerOp {
    LayerOp::Toggle
}

impl Action {
    /// Does nothing when fired: no keys, no text, a macro without a step that does something,
    /// a scroll or relative move of 0, a program, file, profile or layer left blank. A slot
    /// holding an empty action counts as empty: the engine never fires or reports it.
    pub fn is_empty(&self) -> bool {
        match self {
            Action::Keys { keys, .. } => keys.is_empty(),
            Action::Text { text, .. } => text.is_empty(),
            Action::MouseClick { .. } | Action::Engine { .. } => false,
            Action::MouseScroll { dx, dy } => *dx == 0 && *dy == 0,
            Action::MouseMove { x, y, absolute } => !absolute && *x == 0 && *y == 0,
            Action::Sequence { steps } => !steps.iter().any(SequenceStep::has_effect),
            Action::SwitchProfile { target } => {
                matches!(target, ProfileTarget::Profile { id } if id.trim().is_empty())
            }
            Action::Layer { layer, .. } => layer.trim().is_empty(),
            Action::Run { command, .. } => command.trim().is_empty(),
            Action::Open { target } => target.trim().is_empty(),
            Action::Midi { message, .. } => message.is_empty(),
        }
    }
}

/// One step of an [`Action::Sequence`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SequenceStep {
    /// Tap a key combination (see [`Action::Keys`]).
    Keys {
        keys: Vec<Key>,
        #[serde(default)]
        tap_ms: Option<u32>,
    },
    KeyDown { key: Key },
    KeyUp { key: Key },
    Text { text: String },
    Delay { ms: u32 },
    MouseClick {
        button: MouseButton,
        #[serde(default = "default_clicks")]
        clicks: u8,
    },
    MouseDown { button: MouseButton },
    MouseUp { button: MouseButton },
    MouseScroll {
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
    },
    MouseMove {
        #[serde(default)]
        x: i32,
        #[serde(default)]
        y: i32,
        #[serde(default)]
        absolute: bool,
    },
    /// Send a MIDI message (see [`Action::Midi`]).
    Midi {
        #[serde(default)]
        device: Option<String>,
        message: MidiSend,
    },
}

impl SequenceStep {
    /// Produces some input. Empty keys or text, a scroll or relative move of 0 and waits do not
    /// (a macro made only of those is an empty action).
    pub fn has_effect(&self) -> bool {
        match self {
            SequenceStep::Keys { keys, .. } => !keys.is_empty(),
            SequenceStep::Text { text } => !text.is_empty(),
            SequenceStep::Delay { .. } => false,
            SequenceStep::MouseScroll { dx, dy } => *dx != 0 || *dy != 0,
            SequenceStep::MouseMove { x, y, absolute } => *absolute || *x != 0 || *y != 0,
            SequenceStep::Midi { message, .. } => !message.is_empty(),
            SequenceStep::KeyDown { .. }
            | SequenceStep::KeyUp { .. }
            | SequenceStep::MouseClick { .. }
            | SequenceStep::MouseDown { .. }
            | SequenceStep::MouseUp { .. } => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProfileTarget {
    /// The next selectable (non always-on) profile, wrapping around.
    Next,
    Previous,
    Profile { id: String },
    /// The triggering message's value picks the profile: value 0 = the first selectable (non
    /// always-on) profile, 1 = the second, ... Values beyond the last profile do nothing. With
    /// a "any program" Program Change source this makes program N select profile N.
    ByValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum LayerOp {
    On,
    Off,
    Toggle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum EngineOp {
    /// Stop sending output (mappings that control the engine still work).
    Pause,
    Resume,
    TogglePause,
    /// Release every key / button the engine is holding (panic button).
    ReleaseAll,
}

// ---------------------------------------------------------------------------------------------
// Conditions and rate limiting
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition {
    /// The foreground application matches (or, with `negate`, does not match).
    App {
        app: AppMatcher,
        #[serde(default)]
        negate: bool,
    },
    /// A layer is active (`active: true`, the default) or inactive.
    Layer {
        layer: String,
        #[serde(default = "yes")]
        active: bool,
    },
    /// A MIDI note is currently held (`held: true`) or not, on any device.
    /// `channel: None` = any channel. Lets a pad act as a modifier for other controls.
    NoteHeld {
        #[serde(default)]
        channel: Option<u8>,
        note: u8,
        #[serde(default = "yes")]
        held: bool,
    },
    /// The last value of a controller is inside `range`, on any device (a knob or switch
    /// position selecting what other controls do). False if the controller was never received.
    /// Without `range`: the controller is "on" (64..=127, a pedal or switch pressed).
    ControlValue {
        #[serde(default)]
        channel: Option<u8>,
        controller: u8,
        #[serde(default = "on_7bit")]
        range: ValueRange,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum RateLimitMode {
    /// Fire immediately, then swallow everything for `interval_ms`.
    #[default]
    Throttle,
    /// Wait until the input has been quiet for `interval_ms`, then fire once.
    Debounce,
}

/// Limits how often a mapping fires.
///
/// Example: a knob (CC) bound with [`Behavior::Change`] to Ctrl+Shift+O and
/// `RateLimit { mode: Throttle, interval_ms: 500, trailing: false }` sends the hotkey at most
/// once every 500 ms no matter how fast the knob is turned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RateLimit {
    /// Default: throttle.
    #[serde(default)]
    pub mode: RateLimitMode,
    pub interval_ms: u32,
    /// Throttle only: if input was swallowed during the interval, fire once more when the
    /// interval ends (so the final movement is never lost).
    #[serde(default)]
    pub trailing: bool,
}
