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
        "The config file could not be parsed ({error}).{}",
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

/// Parses config JSON (any supported schema version), migrating and repairing it.
pub fn parse(text: &str) -> Result<(Config, Vec<String>), String> {
    let mut value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
    migrate(&mut value)?;
    let mut config: Config = serde_json::from_value(value).map_err(|e| e.to_string())?;
    let warnings = sanitize(&mut config);
    Ok((config, warnings))
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
        Behavior::Trigger { repeat: Some(repeat), .. } => {
            repeat.interval_ms = repeat.interval_ms.max(5);
        }
        _ => {}
    }
    if let Some(limit) = &mut mapping.rate_limit {
        limit.interval_ms = limit.interval_ms.clamp(1, 3_600_000);
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
