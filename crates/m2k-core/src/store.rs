//! Config persistence: load (with migration, repair and corruption recovery) and atomic save.

use std::collections::HashSet;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::model::*;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("could not access the config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("could not serialize the config: {0}")]
    Serialize(#[from] serde_json::Error),
}

/// Result of [`load`].
#[derive(Debug, Clone)]
pub struct Loaded {
    pub config: Config,
    /// No config file existed; this is a fresh default config.
    pub created: bool,
    /// Things the user should be told about (repairs, recovery from a corrupt file, ...).
    pub warnings: Vec<String>,
}

/// The directory holding the config file (`%APPDATA%\midi-2-keyboard`,
/// `~/Library/Application Support/midi-2-keyboard`, `~/.config/midi-2-keyboard`).
/// Overridable with the `M2K_CONFIG_DIR` environment variable.
pub fn default_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("M2K_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    directories::BaseDirs::new()
        .map(|dirs| dirs.config_dir().join("midi-2-keyboard"))
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_path() -> PathBuf {
    default_dir().join("config.json")
}

fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("backup.json")
}

/// Loads the config at `path`. Never fails: a missing file yields the default config, a corrupt
/// file is moved aside and the last backup (or the default config) is used instead.
pub fn load(path: &Path) -> Loaded {
    match fs::read_to_string(path) {
        Ok(text) => match parse(&text) {
            Ok((config, warnings)) => Loaded { config, created: false, warnings },
            Err(error) => recover(path, &error),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Loaded { config: Config::default(), created: true, warnings: Vec::new() }
        }
        Err(error) => Loaded {
            config: Config::default(),
            created: false,
            warnings: vec![format!(
                "Could not read {} ({error}). Using a default configuration; your file was left untouched.",
                path.display()
            )],
        },
    }
}

fn recover(path: &Path, error: &str) -> Loaded {
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let aside = path.with_extension(format!("corrupt-{stamp}.json"));
    let moved = fs::rename(path, &aside).is_ok();
    let mut warnings = vec![format!(
        "The config file {} has a problem and was not loaded: {error}.{}",
        path.display(),
        if moved { format!(" It was kept as {}.", aside.display()) } else { String::new() }
    )];
    if let Ok(text) = fs::read_to_string(backup_path(path))
        && let Ok((config, mut more)) = parse(&text)
    {
        warnings.push("Restored the previous backup instead.".into());
        warnings.append(&mut more);
        return Loaded { config, created: false, warnings };
    }
    warnings.push("Starting from a default configuration.".into());
    Loaded { config: Config::default(), created: false, warnings }
}

/// Parses config JSON (any supported schema version), migrating and repairing it. Errors say
/// what is wrong and where (line and column).
pub fn parse(text: &str) -> Result<(Config, Vec<String>), String> {
    let mut value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    migrate(&mut value)?;
    fill_missing_ranges(&mut value);
    let mut config: Config = match serde_json::from_value(value) {
        Ok(config) => config,
        Err(error) => {
            // A `Value` has no positions; reading the same text again does (the steps above
            // only add what is missing, so it fails the same way).
            let positioned = serde_json::from_str::<Config>(text).err();
            let message = positioned.map_or_else(|| error.to_string(), |e| e.to_string());
            return Err(tidy_error(message));
        }
    };
    let warnings = sanitize(&mut config);
    Ok((config, warnings))
}

/// serde lists every possible name when one is unknown (a misspelled key lists ~180 of them).
/// Keep the message readable: suggest the closest name instead of the whole list.
fn tidy_error(message: String) -> String {
    const LIST: &str = "`, expected one of ";
    let Some(start) = message.find("unknown variant `") else {
        return message;
    };
    let name_start = start + "unknown variant `".len();
    let Some(list_start) = message[name_start..].find(LIST).map(|i| name_start + i) else {
        return message;
    };
    let name = &message[name_start..list_start];
    let rest = &message[list_start + LIST.len()..];
    let (list, position) = match rest.rfind(" at line ") {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, ""),
    };
    let candidates: Vec<&str> = list.split(", ").map(|c| c.trim_matches('`')).collect();
    if candidates.len() <= 8 {
        return message;
    }
    let closest = candidates
        .iter()
        .map(|c| (edit_distance(&name.to_lowercase(), &c.to_lowercase()), *c))
        .min()
        .filter(|(distance, _)| *distance <= (name.chars().count() / 3).max(2));
    let hint = match closest {
        Some((_, suggestion)) => format!("did you mean `{suggestion}`?"),
        None => format!("{} names are possible, such as `{}`", candidates.len(), candidates[0]),
    };
    format!("{}unknown name `{name}` ({hint}){position}", &message[..start])
}

/// Levenshtein distance (small inputs: names).
fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitute = diagonal + usize::from(ca != *cb);
            diagonal = row[j + 1];
            row[j + 1] = substitute.min(row[j] + 1).min(row[j + 1] + 1);
        }
    }
    row[b.len()]
}

/// A ranged, absolute source written without `range` (by hand) gets the range its behavior
/// needs: everything for `change`, `step` and `shuttle`, which follow the value, and the
/// "pressed" half for press/release behaviors, which would otherwise be pressed forever:
/// 64..=127 for 7-bit values, 8192..=16383 for 14-bit ones, and 12288..=16383 for pitch bend
/// (8192 is where the wheel rests). Relative encoders ignore the range and are left alone.
fn fill_missing_ranges(value: &mut Value) {
    let Some(profiles) = value.get_mut("profiles").and_then(Value::as_array_mut) else {
        return;
    };
    for profile in profiles {
        let Some(mappings) = profile.get_mut("mappings").and_then(Value::as_array_mut) else {
            continue;
        };
        for mapping in mappings {
            let follows_value = matches!(
                mapping.pointer("/behavior/type").and_then(Value::as_str),
                Some("change" | "step" | "shuttle")
            );
            let Some(event) = mapping.pointer_mut("/source/event").and_then(Value::as_object_mut)
            else {
                continue;
            };
            if event.contains_key("range") {
                continue;
            }
            let hires = event.get("hires").and_then(Value::as_bool).unwrap_or(false);
            let (pressed, max) = match event.get("type").and_then(Value::as_str) {
                Some("control_change") => {
                    let encoding = event.get("encoding").and_then(Value::as_str);
                    if encoding.is_some_and(|e| e != "absolute") {
                        continue;
                    }
                    (64, 127)
                }
                Some("channel_pressure" | "poly_pressure") => (64, 127),
                Some("nrpn" | "rpn") if !hires => (64, 127),
                Some("nrpn" | "rpn" | "control_change14" | "song_position") => (8_192, 16_383),
                Some("pitch_bend") => (12_288, 16_383),
                _ => continue,
            };
            let min = if follows_value { 0 } else { pressed };
            event.insert("range".into(), serde_json::json!({ "min": min, "max": max }));
        }
    }
}

/// Upgrades older schema versions in place. Add a step here whenever the schema changes in a way
/// `#[serde(default)]` cannot absorb.
fn migrate(value: &mut Value) -> Result<(), String> {
    let version = value.get("version").and_then(Value::as_u64).unwrap_or(1) as u32;
    if version > CONFIG_VERSION {
        return Err(format!(
            "it was written by a newer version of midi-2-keyboard (schema {version}, this build understands {CONFIG_VERSION})"
        ));
    }
    if let Some(object) = value.as_object_mut() {
        object.insert("version".into(), Value::from(CONFIG_VERSION));
    }
    Ok(())
}

/// Repairs anything that would confuse the engine or the UI: missing/duplicate ids, values out
/// of range, inverted ranges, a dangling profile selection. Returns a note per repair.
pub fn sanitize(config: &mut Config) -> Vec<String> {
    let mut notes = Vec::new();
    config.version = CONFIG_VERSION;

    if config.profiles.is_empty() {
        config.profiles = Config::default().profiles;
        notes.push("The configuration had no profiles; a default profile was added.".into());
    }

    let mut profile_ids = HashSet::new();
    let mut mapping_ids = HashSet::new();
    let mut counter = 0u32;
    let mut fresh = |prefix: &str, taken: &HashSet<String>| loop {
        counter += 1;
        let id = format!("{prefix}-{counter:04}");
        if !taken.contains(&id) {
            return id;
        }
    };

    for profile in &mut config.profiles {
        if profile.id.trim().is_empty() || profile_ids.contains(&profile.id) {
            profile.id = fresh("profile", &profile_ids);
        }
        profile_ids.insert(profile.id.clone());
        if profile.name.trim().is_empty() {
            profile.name = "Untitled".into();
        }
        for mapping in &mut profile.mappings {
            if mapping.id.trim().is_empty() || mapping_ids.contains(&mapping.id) {
                mapping.id = fresh("mapping", &mapping_ids);
            }
            mapping_ids.insert(mapping.id.clone());
            sanitize_mapping(mapping);
        }
    }

    let selectable = |id: &String| config.profiles.iter().any(|p| &p.id == id);
    if !config.selected_profile.as_ref().is_some_and(selectable) {
        config.selected_profile = config
            .profiles
            .iter()
            .find(|p| !p.always_on)
            .or(config.profiles.first())
            .map(|p| p.id.clone());
    }

    let mut seen_devices = HashSet::new();
    config.devices.retain(|d| !d.name.is_empty() && seen_devices.insert(d.name.clone()));
    for device in &mut config.devices {
        sanitize_port(&mut device.forward_to);
        // Passing a device on to itself would only send its own messages back to it.
        if device.forward_to.as_deref() == Some(device.name.as_str()) {
            device.forward_to = None;
        }
    }

    let settings = &mut config.settings;
    settings.tap_ms = settings.tap_ms.clamp(1, 5_000);
    settings.key_stagger_ms = settings.key_stagger_ms.min(1_000);
    settings.repeat_delay_ms = settings.repeat_delay_ms.clamp(50, 10_000);
    settings.repeat_interval_ms = settings.repeat_interval_ms.clamp(5, 10_000);
    settings.middle_c_octave = settings.middle_c_octave.clamp(3, 5);

    notes
}

fn sanitize_channel(channel: &mut Option<u8>) {
    if let Some(value) = channel
        && !(1..=16).contains(value)
    {
        *channel = None;
    }
}

fn sanitize_range(range: &mut ValueRange, limit: u16) {
    range.min = range.min.min(limit);
    range.max = range.max.min(limit);
    if range.min > range.max {
        std::mem::swap(&mut range.min, &mut range.max);
    }
}

fn sanitize_mapping(mapping: &mut Mapping) {
    sanitize_channel(&mut mapping.source.channel);
    if mapping.source.device.as_deref() == Some("") {
        mapping.source.device = None;
    }
    mapping.source.hysteresis = mapping.source.hysteresis.min(8_191);
    mapping.source.debounce_ms = mapping.source.debounce_ms.min(2_000);
    match &mut mapping.source.event {
        SourceEvent::Note { notes, velocity } => {
            sanitize_range(notes, 127);
            sanitize_range(velocity, 127);
            velocity.min = velocity.min.max(1);
            velocity.max = velocity.max.max(1);
        }
        SourceEvent::Chord { notes, window_ms } => {
            notes.iter_mut().for_each(|note| *note = (*note).min(127));
            notes.sort_unstable();
            notes.dedup();
            notes.truncate(16);
            *window_ms = (*window_ms).min(10_000);
        }
        SourceEvent::ControlChange { controller, range, .. } => {
            *controller = (*controller).min(127);
            sanitize_range(range, 127);
        }
        SourceEvent::ControlChange14 { controller, range } => {
            *controller = (*controller).min(31);
            sanitize_range(range, 16_383);
        }
        SourceEvent::Nrpn { number, range, hires } | SourceEvent::Rpn { number, range, hires } => {
            *number = (*number).min(16_383);
            sanitize_range(range, if *hires { 16_383 } else { 127 });
        }
        SourceEvent::ProgramChange { programs } => sanitize_range(programs, 127),
        SourceEvent::PitchBend { range } | SourceEvent::SongPosition { range } => {
            sanitize_range(range, 16_383)
        }
        SourceEvent::ChannelPressure { range } => sanitize_range(range, 127),
        SourceEvent::PolyPressure { notes, range } => {
            sanitize_range(notes, 127);
            sanitize_range(range, 127);
        }
        SourceEvent::SongSelect { songs } => sanitize_range(songs, 127),
        SourceEvent::Clock { divisor } => *divisor = (*divisor).max(1),
        SourceEvent::SysEx { .. } | SourceEvent::System { .. } => {}
    }
    for condition in &mut mapping.conditions {
        match condition {
            Condition::NoteHeld { channel, note, .. } => {
                sanitize_channel(channel);
                *note = (*note).min(127);
            }
            Condition::ControlValue { channel, controller, range } => {
                sanitize_channel(channel);
                *controller = (*controller).min(127);
                sanitize_range(range, 127);
            }
            Condition::App { .. } | Condition::Layer { .. } => {}
        }
    }
    match &mut mapping.behavior {
        Behavior::TapHold { hold_ms, .. } => *hold_ms = (*hold_ms).clamp(50, 10_000),
        Behavior::MultiTap { window_ms, hold_ms, .. } => {
            *window_ms = (*window_ms).clamp(50, 5_000);
            *hold_ms = (*hold_ms).clamp(100, 10_000);
        }
        Behavior::Step { step, fast, .. } => {
            *step = (*step).max(1);
            if let Some(fast) = fast {
                fast.ticks = fast.ticks.max(2);
            }
        }
        Behavior::Change { min_change, .. } => *min_change = (*min_change).min(8_191),
        Behavior::Trigger { repeat: Some(repeat), .. } => {
            repeat.interval_ms = repeat.interval_ms.max(5);
        }
        Behavior::Shuttle { dead_zone, slow_ms, fast_ms, .. } => {
            *dead_zone = (*dead_zone).min(90);
            *fast_ms = (*fast_ms).clamp(1, 60_000);
            // The far end is never slower than the near one.
            *slow_ms = (*slow_ms).clamp(*fast_ms, 600_000);
        }
        _ => {}
    }
    if let Some(limit) = &mut mapping.rate_limit {
        limit.interval_ms = limit.interval_ms.clamp(1, 3_600_000);
    }
    if let Some(feedback) = &mut mapping.feedback {
        sanitize_port(&mut feedback.device);
        sanitize_message(&mut feedback.on);
        sanitize_message(&mut feedback.off);
        match &mut feedback.when {
            FeedbackWhen::Layer { layer: name } | FeedbackWhen::Profile { id: name } => {
                if name.trim().len() != name.len() {
                    *name = name.trim().to_string();
                }
            }
            FeedbackWhen::Engaged => {}
        }
    }
    for action in behavior_actions_mut(&mut mapping.behavior) {
        match action {
            Action::Midi { device, message } => sanitize_send(device, message),
            Action::Sequence { steps } => {
                for step in steps {
                    if let SequenceStep::Midi { device, message } = step {
                        sanitize_send(device, message);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Every action a behavior can fire.
fn behavior_actions_mut(behavior: &mut Behavior) -> Vec<&mut Action> {
    match behavior {
        Behavior::Trigger { on_press, on_release, .. } => {
            on_press.iter_mut().chain(on_release.iter_mut()).collect()
        }
        Behavior::Cycle { actions, .. } => actions.iter_mut().collect(),
        Behavior::TapHold { tap, hold, .. } => tap.iter_mut().chain(hold.iter_mut()).collect(),
        Behavior::MultiTap { single, double, triple, hold, .. } => single
            .iter_mut()
            .chain(double.iter_mut())
            .chain(triple.iter_mut())
            .chain(hold.iter_mut())
            .collect(),
        Behavior::Change { action, .. } => vec![action],
        Behavior::Step { up, down, fast, .. } => {
            let mut all: Vec<&mut Action> = up.iter_mut().chain(down.iter_mut()).collect();
            if let Some(fast) = fast {
                all.extend(fast.up.iter_mut().chain(fast.down.iter_mut()));
            }
            all
        }
        Behavior::Shuttle { up, down, .. } => up.iter_mut().chain(down.iter_mut()).collect(),
        Behavior::Hold { .. } | Behavior::Toggle { .. } => Vec::new(),
    }
}

fn sanitize_send(device: &mut Option<String>, message: &mut MidiSend) {
    sanitize_port(device);
    sanitize_message(message);
}

/// An output port name without surrounding blanks (blank = none).
fn sanitize_port(device: &mut Option<String>) {
    if let Some(name) = device {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            *device = None;
        } else if trimmed.len() != name.len() {
            *name = trimmed.to_string();
        }
    }
}

/// Channels 1..=16 and data bytes 0..=127.
fn sanitize_message(message: &mut MidiSend) {
    let fixed = |value: &mut SendValue| {
        if let SendValue::Fixed { value } = value {
            *value = (*value).min(127);
        }
    };
    match message {
        MidiSend::NoteOn { channel, note, velocity } => {
            *channel = (*channel).clamp(1, 16);
            *note = (*note).min(127);
            fixed(velocity);
        }
        MidiSend::NoteOff { channel, note } => {
            *channel = (*channel).clamp(1, 16);
            *note = (*note).min(127);
        }
        MidiSend::ControlChange { channel, controller, value } => {
            *channel = (*channel).clamp(1, 16);
            *controller = (*controller).min(127);
            fixed(value);
        }
        MidiSend::ProgramChange { channel, program } => {
            *channel = (*channel).clamp(1, 16);
            fixed(program);
        }
        MidiSend::Raw { .. } => {}
    }
}

/// Writes the config atomically (temp file + rename), keeping the previous file as a backup.
pub fn save(path: &Path, config: &Config) -> Result<(), StoreError> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(config)?;
    let temp = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&temp)?;
        file.write_all(json.as_bytes())?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }
    if path.exists() {
        // Best effort: a failed backup must not prevent saving.
        let _ = fs::copy(path, backup_path(path));
    }
    fs::rename(&temp, path)?;
    Ok(())
}
