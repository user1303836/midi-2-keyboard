//! `m2k devices` and `m2k monitor` (owned by the MIDI layer).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;

use m2k_midi::input::{InputEvent, InputManager, InputOptions, PortPolicy, PortState, PortStatus};
use m2k_midi::{Derived, MidiMessage, MidiState, ParameterChange, ParameterKind, encode, names};

const DEVICES_USAGE: &str = "usage: m2k devices [--json]

Lists the MIDI input ports and their state. Every port is opened briefly to check that it works.

  --json   print the list as JSON (the PortStatus objects the app uses)";

const MONITOR_USAGE: &str =
    "usage: m2k monitor [port-filter] [--all] [--virtual NAME] [--inject HEX]...

Prints every incoming MIDI message: time in seconds since start, port, decoded message and the
message bytes, followed by the NRPN/RPN and 14-bit CC events it completes (lines starting with
=>). Port changes are printed as they happen. Runs until Ctrl+C.

  port-filter      open only the ports whose name contains this text (case-insensitive);
                   the others stay free for other programs
  --all            also show clock and active sensing (hidden by default)
  --virtual NAME   also create a virtual input port called NAME that other programs can send
                   to (macOS and Linux)
  --inject HEX     decode these bytes as if a port named \"inject\" had received them, print the
                   result and exit without opening any port; repeatable
                   (e.g. --inject \"B0 63 01 B0 62 02 B0 06 40 B0 26 05\")";

/// Name of the port `--inject` pretends the bytes arrived on.
const INJECT_PORT: &str = "inject";

/// Events buffered between the input threads and the terminal before new ones are dropped.
const QUEUE: usize = 8192;

fn options(virtual_port: Option<String>) -> InputOptions {
    InputOptions { client_name: "m2k".into(), virtual_port, ..InputOptions::default() }
}

pub fn devices(args: &[String]) -> i32 {
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "-h" | "--help" => {
                println!("{DEVICES_USAGE}");
                return 0;
            }
            other => {
                eprintln!("m2k devices: unexpected argument {other:?}\n\n{DEVICES_USAGE}");
                return 2;
            }
        }
    }

    let manager = InputManager::start(options(None), PortPolicy::All, |_| {});
    let ports = manager.ports();
    drop(manager);

    if json {
        return match serde_json::to_string_pretty(&ports) {
            Ok(text) => {
                println!("{text}");
                0
            }
            Err(error) => {
                eprintln!("m2k devices: {error}");
                1
            }
        };
    }
    if ports.is_empty() {
        println!("No MIDI input ports found.");
        return 0;
    }
    let width = ports.iter().map(|p| p.name.chars().count()).max().unwrap_or(0);
    let plural = if ports.len() == 1 { "" } else { "s" };
    println!("{} MIDI input port{plural}:", ports.len());
    for port in &ports {
        println!("  {:<width$}  {}", port.name, describe_state(port));
    }
    0
}

pub fn monitor(args: &[String]) -> i32 {
    let mut filter: Option<String> = None;
    let mut show_all = false;
    let mut virtual_port: Option<String> = None;
    let mut injections: Vec<Vec<u8>> = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-a" | "--all" => show_all = true,
            "--virtual" => match args.next() {
                Some(name) => virtual_port = Some(name.clone()),
                None => return usage_error("--virtual needs a port name"),
            },
            "--inject" => match args.next().map(|hex| parse_hex(hex)) {
                Some(Ok(bytes)) => injections.push(bytes),
                Some(Err(error)) => return usage_error(&format!("--inject: {error}")),
                None => return usage_error("--inject needs hex bytes"),
            },
            "-h" | "--help" => {
                println!("{MONITOR_USAGE}");
                return 0;
            }
            flag if flag.starts_with('-') => {
                return usage_error(&format!("unexpected argument {flag:?}"));
            }
            text if filter.is_none() => filter = Some(text.to_lowercase()),
            text => return usage_error(&format!("unexpected argument {text:?} (one filter only)")),
        }
    }
    if virtual_port.is_some() && cfg!(windows) {
        eprintln!("note: Windows has no virtual MIDI ports; --virtual is ignored");
    }

    let (tx, rx) = mpsc::sync_channel::<InputEvent>(QUEUE);
    let dropped = Arc::new(AtomicU64::new(0));
    let sink = {
        let dropped = dropped.clone();
        move |event| {
            if tx.try_send(event).is_err() {
                dropped.fetch_add(1, Ordering::Relaxed);
            }
        }
    };

    if !injections.is_empty() {
        let manager = InputManager::start(options(None), PortPolicy::Only(Vec::new()), sink);
        for bytes in &injections {
            manager.inject(INJECT_PORT, bytes);
        }
        drop(manager);
        let mut monitor = Monitor::new(true, None);
        for event in rx.try_iter() {
            if let InputEvent::Message { port, time_us, message } = event {
                monitor.message(&port, time_us, &message);
            }
        }
        let drops = dropped.load(Ordering::Relaxed);
        if drops > 0 {
            println!("({drops} events dropped: more than {QUEUE} at once)");
        }
        return 0;
    }

    // With a filter, nothing is opened until the first port list shows what matches.
    let policy = if filter.is_some() { PortPolicy::Only(Vec::new()) } else { PortPolicy::All };
    let manager = InputManager::start(options(virtual_port), policy, sink);
    let what = match &filter {
        Some(text) => format!("MIDI input ports matching {text:?}"),
        None => "all MIDI input ports".to_owned(),
    };
    let hidden =
        if show_all { "" } else { " Clock and active sensing are hidden (--all shows them)." };
    println!("Monitoring {what}. Press Ctrl+C to stop.{hidden}");

    let mut monitor = Monitor::new(show_all, filter.clone());
    let mut opened: Option<Vec<String>> = None;
    let mut reported_drops = 0;
    for event in rx.iter() {
        match event {
            InputEvent::PortsChanged { ports } => {
                monitor.ports_changed(&ports);
                if let Some(text) = &filter {
                    let wanted: Vec<String> = ports
                        .iter()
                        .filter(|p| p.is_virtual || matches_filter(&p.name, text))
                        .map(|p| p.name.clone())
                        .collect();
                    if opened.as_ref() != Some(&wanted) {
                        manager.set_policy(PortPolicy::Only(wanted.clone()));
                        opened = Some(wanted);
                    }
                }
            }
            InputEvent::Message { port, time_us, message } => {
                monitor.message(&port, time_us, &message);
            }
        }
        let drops = dropped.load(Ordering::Relaxed);
        if drops > reported_drops {
            println!("({} events dropped: the terminal cannot keep up)", drops - reported_drops);
            reported_drops = drops;
        }
    }
    0
}

fn usage_error(message: &str) -> i32 {
    eprintln!("m2k monitor: {message}\n\n{MONITOR_USAGE}");
    2
}

fn matches_filter(name: &str, lowercase_filter: &str) -> bool {
    name.to_lowercase().contains(lowercase_filter)
}

/// Parses `"90 3C 64"`, `"903C64"`, `"0x90,0x3c,0x64"`, ... (also used by `m2k send`).
pub(crate) fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    for token in text.split(|c: char| c.is_whitespace() || c == ',').filter(|t| !t.is_empty()) {
        let digits = token.strip_prefix("0x").or_else(|| token.strip_prefix("0X")).unwrap_or(token);
        let valid = !digits.is_empty()
            && (digits.len() == 1 || digits.len() % 2 == 0)
            && digits.bytes().all(|b| b.is_ascii_hexdigit());
        if !valid {
            return Err(format!("{token:?} is not a hex byte sequence"));
        }
        if digits.len() == 1 {
            bytes.push(u8::from_str_radix(digits, 16).map_err(|e| e.to_string())?);
            continue;
        }
        for pair in digits.as_bytes().chunks(2) {
            let pair = std::str::from_utf8(pair).map_err(|e| e.to_string())?;
            bytes.push(u8::from_str_radix(pair, 16).map_err(|e| e.to_string())?);
        }
    }
    if bytes.is_empty() {
        return Err("no bytes given".into());
    }
    Ok(bytes)
}

struct Monitor {
    show_all: bool,
    filter: Option<String>,
    /// Per-port state, to show the NRPN/RPN and 14-bit events each message completes.
    states: HashMap<Arc<str>, MidiState>,
    /// The ports shown so far, with the state that was printed.
    printed: BTreeMap<String, (PortState, String)>,
    first_report: bool,
    width: usize,
}

impl Monitor {
    fn new(show_all: bool, filter: Option<String>) -> Self {
        Monitor {
            show_all,
            filter,
            states: HashMap::new(),
            printed: BTreeMap::new(),
            first_report: true,
            width: INJECT_PORT.len(),
        }
    }

    fn shown(&self, port: &PortStatus) -> bool {
        port.is_virtual || self.filter.as_deref().is_none_or(|f| matches_filter(&port.name, f))
    }

    fn ports_changed(&mut self, ports: &[PortStatus]) {
        // Disabled ports are the ones the filter keeps closed (or has not opened yet).
        let visible: Vec<&PortStatus> =
            ports.iter().filter(|p| self.shown(p) && p.state != PortState::Disabled).collect();
        for port in &visible {
            match self.printed.get(&port.name) {
                None => println!("+ port {}: {}", port.name, describe_state(port)),
                Some((state, error)) if *state != port.state || *error != port.error => {
                    println!("~ port {}: {}", port.name, describe_state(port));
                }
                Some(_) => {}
            }
        }
        for name in self.printed.keys() {
            if !visible.iter().any(|p| &p.name == name) {
                let present = ports.iter().any(|p| &p.name == name);
                println!("- port {name}: {}", if present { "closed" } else { "removed" });
            }
        }
        self.printed =
            visible.iter().map(|p| (p.name.clone(), (p.state, p.error.clone()))).collect();
        // A port that is gone or closed starts from scratch when it comes back.
        self.states.retain(|name, _| {
            &**name == INJECT_PORT
                || ports.iter().any(|p| *p.name == **name && p.state == PortState::Connected)
        });
        if self.first_report && !ports.iter().any(|p| self.shown(p)) {
            let which = if self.filter.is_some() { " matching the filter" } else { "" };
            println!("No MIDI input ports{which} yet; waiting for one to be connected.");
        }
        let longest = ports.iter().map(|p| p.name.chars().count()).max().unwrap_or(0);
        self.width = self.width.max(longest.min(40));
        self.first_report = false;
    }

    fn message(&mut self, port: &Arc<str>, time_us: u64, message: &MidiMessage) {
        let derived = self.states.entry(port.clone()).or_default().update(message);
        if message.is_noise() && !self.show_all {
            return;
        }
        self.width = self.width.max(port.chars().count().min(40));
        let width = self.width;
        let time = format!("{}.{:06}", time_us / 1_000_000, time_us % 1_000_000);
        println!(
            "{time:>12}  {port:<width$}  {:<44}  {}",
            describe_message(message),
            hex(&encode(message))
        );
        for event in derived.iter() {
            println!("{:>12}  {:<width$}  => {}", "", "", describe_derived(&event));
        }
    }
}

pub(crate) fn describe_state(port: &PortStatus) -> String {
    let state = match port.state {
        PortState::Connected => "connected".to_owned(),
        PortState::Disabled => "disabled".to_owned(),
        PortState::Error => format!("error: {}", port.error),
    };
    if port.is_virtual { format!("{state} (our virtual port)") } else { state }
}

/// Space separated hex, shortened for long SysEx.
pub(crate) fn hex(bytes: &[u8]) -> String {
    const SHOWN: usize = 24;
    let mut text: Vec<String> = bytes.iter().take(SHOWN).map(|b| format!("{b:02X}")).collect();
    if bytes.len() > SHOWN {
        text.push(format!("... ({} bytes)", bytes.len()));
    }
    text.join(" ")
}

fn note(note: u8) -> String {
    format!("{} ({note})", names::note_name(note, 4))
}

pub(crate) fn describe_message(message: &MidiMessage) -> String {
    match *message {
        MidiMessage::NoteOn { channel, note: n, velocity: 0 } => {
            format!("ch{channel:<2} Note On    {} vel 0 (= off)", note(n))
        }
        MidiMessage::NoteOn { channel, note: n, velocity } => {
            format!("ch{channel:<2} Note On    {} vel {velocity}", note(n))
        }
        MidiMessage::NoteOff { channel, note: n, velocity } => {
            format!("ch{channel:<2} Note Off   {} vel {velocity}", note(n))
        }
        MidiMessage::PolyPressure { channel, note: n, pressure } => {
            format!("ch{channel:<2} Poly AT    {} = {pressure}", note(n))
        }
        MidiMessage::ControlChange { channel, controller, value } => match cc_name(controller) {
            "" => format!("ch{channel:<2} CC {controller} = {value}"),
            name => format!("ch{channel:<2} CC {controller} {name} = {value}"),
        },
        MidiMessage::ProgramChange { channel, program } => {
            format!("ch{channel:<2} Program    {program}")
        }
        MidiMessage::ChannelPressure { channel, pressure } => {
            format!("ch{channel:<2} Channel AT {pressure}")
        }
        MidiMessage::PitchBend { channel, value } => {
            format!("ch{channel:<2} Pitch Bend {value} ({:+})", i32::from(value) - 8192)
        }
        MidiMessage::SysEx { ref data } => format!("SysEx ({})", byte_count(data.len())),
        MidiMessage::MtcQuarterFrame { piece, value } => {
            format!("MTC Quarter Frame {piece}: {value}")
        }
        MidiMessage::SongPosition { position } => format!("Song Position {position}"),
        MidiMessage::SongSelect { song } => format!("Song Select {song}"),
        MidiMessage::TuneRequest => "Tune Request".into(),
        MidiMessage::Clock => "Clock".into(),
        MidiMessage::Start => "Start".into(),
        MidiMessage::Continue => "Continue".into(),
        MidiMessage::Stop => "Stop".into(),
        MidiMessage::ActiveSensing => "Active Sensing".into(),
        MidiMessage::Reset => "System Reset".into(),
        MidiMessage::Unknown { ref data } => format!("Unknown ({})", byte_count(data.len())),
    }
}

fn byte_count(n: usize) -> String {
    if n == 1 { "1 byte".to_owned() } else { format!("{n} bytes") }
}

pub(crate) fn describe_derived(event: &Derived) -> String {
    match *event {
        Derived::Parameter(p) => {
            let kind = match p.kind {
                ParameterKind::Nrpn => "NRPN",
                ParameterKind::Rpn => "RPN",
            };
            let name = match p.kind {
                ParameterKind::Rpn if !rpn_name(p.number).is_empty() => {
                    format!(" {}", rpn_name(p.number))
                }
                _ => String::new(),
            };
            let change = match p.change {
                ParameterChange::DataMsb => "data MSB",
                ParameterChange::DataLsb => "data LSB",
                ParameterChange::Increment => "increment",
                ParameterChange::Decrement => "decrement",
            };
            format!(
                "ch{:<2} {kind} {} ({}/{}){name}: {change} -> {} (14-bit {})",
                p.channel,
                p.number,
                p.number >> 7,
                p.number & 0x7F,
                p.value7(),
                p.value14()
            )
        }
        Derived::ControlChange14(c) => format!(
            "ch{:<2} CC {}/{} (14-bit) = {}",
            c.channel,
            c.controller,
            c.controller + 32,
            c.value
        ),
    }
}

/// Conventional controller names (the same table as the UI's `ccName`).
fn cc_name(controller: u8) -> &'static str {
    match controller {
        0 => "Bank Select",
        1 => "Mod Wheel",
        2 => "Breath",
        4 => "Foot Controller",
        5 => "Portamento Time",
        6 => "Data Entry",
        7 => "Volume",
        8 => "Balance",
        10 => "Pan",
        11 => "Expression",
        12 => "Effect 1",
        13 => "Effect 2",
        32 => "Bank Select LSB",
        38 => "Data Entry LSB",
        64 => "Sustain Pedal",
        65 => "Portamento",
        66 => "Sostenuto",
        67 => "Soft Pedal",
        68 => "Legato",
        69 => "Hold 2",
        70 => "Sound Variation",
        71 => "Resonance",
        72 => "Release",
        73 => "Attack",
        74 => "Cutoff",
        84 => "Portamento Control",
        91 => "Reverb",
        92 => "Tremolo",
        93 => "Chorus",
        94 => "Detune",
        95 => "Phaser",
        96 => "Data Increment",
        97 => "Data Decrement",
        98 => "NRPN LSB",
        99 => "NRPN MSB",
        100 => "RPN LSB",
        101 => "RPN MSB",
        120 => "All Sound Off",
        121 => "Reset Controllers",
        122 => "Local Control",
        123 => "All Notes Off",
        124 => "Omni Off",
        125 => "Omni On",
        126 => "Mono On",
        127 => "Poly On",
        _ => "",
    }
}

/// Registered parameter names (the same table as the UI's `rpnName`).
fn rpn_name(number: u16) -> &'static str {
    match number {
        0 => "Pitch Bend Range",
        1 => "Fine Tuning",
        2 => "Coarse Tuning",
        3 => "Tuning Program",
        4 => "Tuning Bank",
        5 => "Mod Depth Range",
        6 => "MPE Configuration",
        _ => "",
    }
}
