//! `m2k simulate` (owned by the engine): run a config against scripted MIDI input with a logging
//! injector and print everything the engine would send.
//!
//! The script runs in real time through a real engine. Every call the engine makes to the
//! output backend is printed with a millisecond timestamp, as is every engine event. At the end
//! the engine is stopped and the tool checks that nothing was left held.

use std::collections::{BTreeSet, HashMap};
use std::fmt::Display;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use m2k_core::{
    Action, Config, Engine, EngineEvent, EngineOptions, EngineStatus, MidiSink, Outcome, SkipReason,
};
use m2k_midi::{Derived, MidiMessage, ParameterChange, ParameterKind, Parser, names::note_name};
use m2k_output::platform::ForegroundApp;
use m2k_output::{Injector, Key, MouseButton, OutputError};

const USAGE: &str = "usage: m2k simulate <config.json> [script | -] [options]

Runs the config against a script of timed input (read from stdin when no script is given or
it is -) and prints every backend call and engine event with a millisecond timestamp. MIDI the
engine sends is printed as MIDI-OUT lines; every output port exists unless --absent-output says
otherwise.

script lines (a time prefix is optional: +MS = after the previous line, @MS = since start):
  +0 \"Device A\" 90 3C 64        MIDI bytes from a device (running status works)
  wait 600                       let time pass
  foreground \"Photoshop.exe\" \"Untitled-1\"   |   foreground none
  pause | resume | focus on | focus off | learn on | learn off
  select <profile-id>             select a profile
  disconnect \"Device A\"           the device went away
  release-all                     the panic button
  config <file.json>              hot-swap the config
  run [delay-ms] <action json>    the UI's Test button
  status                          print the engine status snapshot
  output-gone \"Port\"              that MIDI output disappears
  output-back \"Port\"              it is there again (and the engine is told: lights refresh)
  outputs-changed \"Port\" ...      tell the engine these outputs changed
  inputs \"Device\" ...               the MIDI inputs connected now (where unaimed lights go)
  swap-backend                    replace the output backend (set_injector)
  echo <text>                     print a marker line
  # comment

options:
  --json            print engine events as JSON
  --repeats         the backend auto-repeats held keys by itself (like X11 / uinput)
  --fail <Key>      the backend rejects this key (repeatable), e.g. --fail F13
  --panic <Key>     the backend panics when this key goes down (tests recovery)
  --fail-up <Key>[:N]  the backend refuses the first N (default 1) releases of this key
  --as-backend <name>  report this backend name
  --text-chunk <N>  the backend takes at most N characters per text call (like linux-uinput)
  --settle <ms>     wait this long after the script before stopping (default 1000)
  --clock           also show MIDI clock / active sensing messages
  --track-foreground  poll the real foreground application (default: only `foreground` lines)
  --panic-sink <N>  the event sink panics on its Nth event (tests that the engine survives)
  --absent-output <name>  there is no MIDI output port of this name (repeatable)
  --quiet-status    do not print status events";

fn say(line: std::fmt::Arguments<'_>) {
    use std::io::Write;
    let _ = writeln!(std::io::stdout().lock(), "{line}");
}

struct Clock(Instant);

impl Clock {
    fn ms(&self) -> f64 {
        self.0.elapsed().as_secs_f64() * 1000.0
    }

    /// Prints one line. Write errors are ignored: when the reader goes away (`| head`), the
    /// simulation still runs to its end and stops the engine cleanly.
    fn print(&self, tag: &str, text: impl Display) {
        say(format_args!("{:>9.1}  {tag:<5}  {text}", self.ms()));
    }
}

#[derive(Clone, Copy, Default)]
struct PrintOptions {
    json: bool,
    clock: bool,
    quiet_status: bool,
}

#[derive(Default)]
struct Held {
    keys: BTreeSet<Key>,
    buttons: BTreeSet<MouseButton>,
    key_downs: u64,
    key_ups: u64,
}

#[derive(Default)]
struct Tally {
    sent: u64,
    skipped: u64,
    failed: u64,
    errors: u64,
}

/// The MIDI output of the simulation: prints what the engine sends.
struct LogMidiSink {
    clock: Arc<Clock>,
    absent: Arc<Mutex<BTreeSet<String>>>,
    /// Devices switched off in the config (`devices[].enabled: false`): left alone like the app
    /// leaves them (outputs of the same name; the app also pairs MIDIINn / MIDIOUTn names).
    disabled: Arc<Mutex<BTreeSet<String>>>,
    sent: Arc<std::sync::atomic::AtomicU64>,
}

fn switched_off(config: &Config) -> BTreeSet<String> {
    config
        .devices
        .iter()
        .filter(|device| !device.enabled)
        .map(|device| device.name.clone())
        .collect()
}

impl MidiSink for LogMidiSink {
    fn send(&self, port: &str, bytes: &[u8]) -> Result<(), String> {
        if !self.has_port(port) {
            return Err(format!("there is no MIDI output named \"{port}\""));
        }
        let hex: Vec<String> = bytes.iter().map(|b| format!("{b:02X}")).collect();
        self.clock.print("MIDI-OUT", format_args!("\"{port}\" {}", hex.join(" ")));
        self.sent.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    fn has_port(&self, name: &str) -> bool {
        !self.absent.lock().unwrap_or_else(|e| e.into_inner()).contains(name)
    }

    fn unavailable(&self, port: &str) -> Option<String> {
        if self.disabled.lock().unwrap_or_else(|e| e.into_inner()).contains(port) {
            return Some(format!("\u{201C}{port}\u{201D} is switched off under MIDI devices"));
        }
        (!self.has_port(port)).then(|| format!("there is no MIDI output named \"{port}\""))
    }
}

/// The output backend of the simulation: prints and records what it is asked to do.
struct LogInjector {
    name: &'static str,
    clock: Arc<Clock>,
    held: Arc<Mutex<Held>>,
    repeats: bool,
    fail: Vec<Key>,
    /// Pressing this key panics inside the backend (exercises the engine's panic recovery).
    panic: Option<Key>,
    /// Releases of these keys are refused this many more times (like Windows UIPI while an
    /// elevated window has focus). Shared by all backends of the run.
    fail_up: Arc<Mutex<HashMap<Key, u32>>>,
    text_chunk: Option<usize>,
}

impl Injector for LogInjector {
    fn name(&self) -> &'static str {
        self.name
    }

    fn supports(&self, key: Key) -> bool {
        !self.fail.contains(&key)
    }

    fn repeats_held_keys(&self) -> bool {
        self.repeats
    }

    fn max_text_chunk(&self) -> Option<usize> {
        self.text_chunk
    }

    fn key_down(&mut self, key: Key) -> Result<(), OutputError> {
        if self.panic == Some(key) {
            self.clock.print("KEY", format!("down {key:?}  -> backend panics (--panic)"));
            panic!("simulated backend panic on {key:?}");
        }
        if self.fail.contains(&key) {
            self.clock.print("KEY", format!("down {key:?}  -> rejected (--fail)"));
            return Err(OutputError::Unsupported(key.code(), "simulate"));
        }
        let again = {
            let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
            held.key_downs += 1;
            !held.keys.insert(key)
        };
        self.clock.print("KEY", format!("down {key:?}{}", if again { "  (again)" } else { "" }));
        Ok(())
    }

    fn key_up(&mut self, key: Key) -> Result<(), OutputError> {
        {
            let mut fail_up = self.fail_up.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(left) = fail_up.get_mut(&key)
                && *left > 0
            {
                *left -= 1;
                drop(fail_up);
                self.clock
                    .print("KEY", format!("up   {key:?}  -> refused (--fail-up), still down"));
                return Err(OutputError::Permission("blocked by the simulation".into()));
            }
        }
        let was_down = {
            let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
            held.key_ups += 1;
            held.keys.remove(&key)
        };
        self.clock.print(
            "KEY",
            format!("up   {key:?}{}", if was_down { "" } else { "  (was not down)" }),
        );
        Ok(())
    }

    fn text(&mut self, text: &str) -> Result<(), OutputError> {
        self.clock.print("TEXT", format!("{text:?}"));
        Ok(())
    }

    fn mouse_button(&mut self, button: MouseButton, down: bool) -> Result<(), OutputError> {
        {
            let mut held = self.held.lock().unwrap_or_else(|e| e.into_inner());
            if down {
                held.buttons.insert(button);
            } else {
                held.buttons.remove(&button);
            }
        }
        self.clock.print("MOUSE", format!("{} {button:?}", if down { "down" } else { "up  " }));
        Ok(())
    }

    fn mouse_move_relative(&mut self, dx: i32, dy: i32) -> Result<(), OutputError> {
        self.clock.print("MOUSE", format!("move by ({dx}, {dy})"));
        Ok(())
    }

    fn mouse_move_absolute(&mut self, x: i32, y: i32) -> Result<(), OutputError> {
        self.clock.print("MOUSE", format!("move to ({x}, {y})"));
        Ok(())
    }

    fn scroll(&mut self, dx: i32, dy: i32) -> Result<(), OutputError> {
        self.clock.print("MOUSE", format!("scroll dx={dx} dy={dy}"));
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Script
// ---------------------------------------------------------------------------------------------

enum Step {
    Midi { device: String, bytes: Vec<u8> },
    Foreground(Option<ForegroundApp>),
    Paused(bool),
    Focused(bool),
    Learning(bool),
    Select(String),
    Disconnect(String),
    ReleaseAll,
    OutputGone(String),
    OutputBack(String),
    OutputsChanged(Vec<String>),
    Inputs(Vec<String>),
    Config(PathBuf),
    Run { delay_ms: u32, action: Box<Action> },
    Status,
    SwapBackend,
    Echo,
    Wait,
}

struct Line {
    at_ms: u64,
    text: String,
    step: Step,
}

/// Splits on whitespace; double-quoted parts (with `\"` escapes) are one token.
fn tokenize(text: &str) -> Result<Vec<String>, String> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut token = String::new();
        if c == '"' {
            chars.next();
            let mut closed = false;
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(next) = chars.next() {
                            token.push(next);
                        }
                    }
                    '"' => {
                        closed = true;
                        break;
                    }
                    c => token.push(c),
                }
            }
            if !closed {
                return Err("unterminated quote".into());
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    break;
                }
                token.push(c);
                chars.next();
            }
        }
        tokens.push(token);
    }
    Ok(tokens)
}

fn parse_hex(tokens: &[String]) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    for token in tokens.iter().flat_map(|t| t.split(',')).filter(|t| !t.is_empty()) {
        let digits = token.strip_prefix("0x").or_else(|| token.strip_prefix("0X")).unwrap_or(token);
        let byte =
            u8::from_str_radix(digits, 16).map_err(|_| format!("\"{token}\" is not a hex byte"))?;
        bytes.push(byte);
    }
    if bytes.is_empty() {
        return Err("no MIDI bytes".into());
    }
    Ok(bytes)
}

fn on_off(token: Option<&String>) -> Result<bool, String> {
    match token.map(String::as_str) {
        Some("on" | "true" | "1") => Ok(true),
        Some("off" | "false" | "0") => Ok(false),
        _ => Err("expected on or off".into()),
    }
}

fn parse_line(rest: &str, now: &mut u64) -> Result<Step, String> {
    // `run` takes raw JSON, which the tokenizer would mangle.
    if let Some(args) = rest.strip_prefix("run ") {
        let args = args.trim_start();
        let (delay_ms, json) = match args.split_once(char::is_whitespace) {
            Some((first, json)) if first.parse::<u32>().is_ok() => {
                (first.parse().unwrap_or(0), json)
            }
            _ => (0, args),
        };
        let action: Action =
            serde_json::from_str(json).map_err(|e| format!("bad action JSON: {e}"))?;
        return Ok(Step::Run { delay_ms, action: Box::new(action) });
    }
    let tokens = tokenize(rest)?;
    let Some(first) = tokens.first() else {
        return Ok(Step::Wait);
    };
    let arg = |i: usize| tokens.get(i).cloned().ok_or_else(|| format!("{first}: missing argument"));
    Ok(match first.as_str() {
        _ if rest.starts_with('"') => {
            Step::Midi { device: first.clone(), bytes: parse_hex(&tokens[1..])? }
        }
        "midi" => Step::Midi { device: arg(1)?, bytes: parse_hex(&tokens[2..])? },
        "wait" => {
            *now += arg(1)?.parse::<u64>().map_err(|_| "wait: expected milliseconds")?;
            Step::Wait
        }
        "foreground" => match tokens.get(1).map(String::as_str) {
            None | Some("none") => Step::Foreground(None),
            Some(process) => Step::Foreground(Some(ForegroundApp {
                process: process.to_string(),
                title: tokens.get(2).cloned().unwrap_or_default(),
                app_id: String::new(),
                pid: 0,
                elevated: false,
            })),
        },
        "pause" => Step::Paused(true),
        "resume" => Step::Paused(false),
        "focus" => Step::Focused(on_off(tokens.get(1))?),
        "learn" => Step::Learning(on_off(tokens.get(1))?),
        "select" => Step::Select(arg(1)?),
        "disconnect" => Step::Disconnect(arg(1)?),
        "release-all" => Step::ReleaseAll,
        "output-gone" => Step::OutputGone(arg(1)?),
        "output-back" => Step::OutputBack(arg(1)?),
        "outputs-changed" => Step::OutputsChanged(tokens[1..].to_vec()),
        "inputs" => Step::Inputs(tokens[1..].to_vec()),
        "config" => Step::Config(PathBuf::from(arg(1)?)),
        "status" => Step::Status,
        "swap-backend" => Step::SwapBackend,
        "echo" => Step::Echo,
        other => return Err(format!("unknown directive \"{other}\"")),
    })
}

fn parse_script(text: &str) -> Result<Vec<Line>, String> {
    let mut lines = Vec::new();
    let mut now = 0u64;
    for (i, raw) in text.lines().enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let fail = |e: String| format!("script line {}: {e}\n  {trimmed}", i + 1);
        let mut rest = trimmed;
        for (prefix, absolute) in [('+', false), ('@', true)] {
            if let Some(after) = rest.strip_prefix(prefix) {
                let end = after.find(|c: char| !c.is_ascii_digit()).unwrap_or(after.len());
                let ms: u64 = after[..end].parse().map_err(|_| fail("bad time prefix".into()))?;
                if absolute {
                    if ms < now {
                        return Err(fail(format!(
                            "@{ms} is earlier than the previous line ({now} ms)"
                        )));
                    }
                    now = ms;
                } else {
                    now += ms;
                }
                rest = after[end..].trim_start();
                break;
            }
        }
        let step = parse_line(rest, &mut now).map_err(fail)?;
        lines.push(Line { at_ms: now, text: trimmed.to_string(), step });
    }
    Ok(lines)
}

// ---------------------------------------------------------------------------------------------
// Printing
// ---------------------------------------------------------------------------------------------

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

fn describe_message(message: &MidiMessage) -> String {
    let note = |n: u8| format!("{} ({n})", note_name(n, 4));
    match message {
        MidiMessage::NoteOn { channel, note: n, velocity: 0 } => {
            format!("note on  ch{channel} {} vel 0 (= off)", note(*n))
        }
        MidiMessage::NoteOn { channel, note: n, velocity } => {
            format!("note on  ch{channel} {} vel {velocity}", note(*n))
        }
        MidiMessage::NoteOff { channel, note: n, velocity } => {
            format!("note off ch{channel} {} vel {velocity}", note(*n))
        }
        MidiMessage::PolyPressure { channel, note: n, pressure } => {
            format!("poly pressure ch{channel} {} = {pressure}", note(*n))
        }
        MidiMessage::ControlChange { channel, controller, value } => {
            format!("cc ch{channel} #{controller} = {value}")
        }
        MidiMessage::ProgramChange { channel, program } => format!("program ch{channel} {program}"),
        MidiMessage::ChannelPressure { channel, pressure } => {
            format!("channel pressure ch{channel} = {pressure}")
        }
        MidiMessage::PitchBend { channel, value } => format!("pitch bend ch{channel} = {value}"),
        MidiMessage::SysEx { data } => format!("sysex {}", hex(data)),
        MidiMessage::Unknown { data } => format!("unknown {}", hex(data)),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn describe_derived(derived: &Derived) -> String {
    match derived {
        Derived::Parameter(p) => {
            let kind = match p.kind {
                ParameterKind::Nrpn => "nrpn",
                ParameterKind::Rpn => "rpn",
            };
            let change = match p.change {
                ParameterChange::DataMsb => "data msb",
                ParameterChange::DataLsb => "data lsb",
                ParameterChange::Increment => "increment",
                ParameterChange::Decrement => "decrement",
            };
            format!(
                "{kind} ch{} #{} {change}: msb {} lsb {} (14-bit {})",
                p.channel,
                p.number,
                p.msb,
                p.lsb,
                p.value14()
            )
        }
        Derived::ControlChange14(c) => {
            format!("cc14 ch{} #{} = {}", c.channel, c.controller, c.value)
        }
    }
}

fn describe_status(status: &EngineStatus) -> String {
    let mut text = format!(
        "profile {}{}",
        status.effective_profile.as_deref().unwrap_or("-"),
        if status.auto_switched { " (auto)" } else { "" }
    );
    if status.selected_profile != status.effective_profile {
        text.push_str(&format!(
            " [selected {}]",
            status.selected_profile.as_deref().unwrap_or("-")
        ));
    }
    let keys: Vec<String> = status.held_keys.iter().map(|k| format!("{k:?}")).collect();
    text.push_str(&format!("  held [{}]", keys.join(" ")));
    text.push_str(&format!("  engaged [{}]", status.engaged_mappings.join(" ")));
    if !status.layers.is_empty() {
        text.push_str(&format!("  layers [{}]", status.layers.join(" ")));
    }
    for (flag, name) in [
        (status.paused, "PAUSED"),
        (status.focus_suspended, "FOCUS-SUSPENDED"),
        (status.learning, "LEARNING"),
    ] {
        if flag {
            text.push_str("  ");
            text.push_str(name);
        }
    }
    if let Some(latency) = &status.latency {
        text.push_str(&format!(
            "  latency n={} median {:.2} ms p99 {:.2} ms max {:.2} ms",
            latency.samples,
            f64::from(latency.median_us) / 1000.0,
            f64::from(latency.p99_us) / 1000.0,
            f64::from(latency.max_us) / 1000.0
        ));
    }
    if let Some(app) = &status.foreground {
        text.push_str(&format!("  fg {:?}", app.process));
    }
    text
}

fn skip_reason(reason: SkipReason) -> &'static str {
    match reason {
        SkipReason::Paused => "paused",
        SkipReason::WindowFocused => "window focused",
        SkipReason::Learning => "learning",
        SkipReason::RateLimited => "rate limited",
        SkipReason::ConditionFailed => "condition failed",
    }
}

fn print_event(clock: &Clock, options: PrintOptions, tally: &Mutex<Tally>, event: &EngineEvent) {
    {
        let mut tally = tally.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            EngineEvent::Fired(fired) => match fired.outcome {
                Outcome::Sent => tally.sent += 1,
                Outcome::Skipped { .. } => tally.skipped += 1,
                Outcome::Failed { .. } => tally.failed += 1,
            },
            EngineEvent::Error { .. } => tally.errors += 1,
            _ => {}
        }
    }
    if let EngineEvent::Midi(log) = event
        && log.message.is_noise()
        && !options.clock
    {
        return;
    }
    if matches!(event, EngineEvent::Status(_)) && options.quiet_status {
        return;
    }
    if options.json {
        clock.print("EVENT", serde_json::to_string(event).unwrap_or_default());
        return;
    }
    match event {
        EngineEvent::Midi(log) => {
            let mut text = format!("{:?} {}", log.device, describe_message(&log.message));
            for derived in &log.derived {
                text.push_str(&format!("  [{}]", describe_derived(derived)));
            }
            if !log.mappings.is_empty() {
                text.push_str(&format!("  -> {}", log.mappings.join(", ")));
            }
            clock.print("MIDI", text);
        }
        EngineEvent::Fired(fired) => {
            let outcome = match &fired.outcome {
                Outcome::Sent => "sent".to_string(),
                Outcome::Skipped { reason, detail } if detail.is_empty() => {
                    format!("skipped: {}", skip_reason(*reason))
                }
                Outcome::Skipped { reason, detail } => {
                    format!("skipped: {}: {detail}", skip_reason(*reason))
                }
                Outcome::Failed { error } => format!("FAILED: {error}"),
            };
            let who =
                if fired.mapping_id.is_empty() { "(test)" } else { fired.mapping_id.as_str() };
            clock.print("FIRE", format!("{who}: {}  [{outcome}]", fired.description));
        }
        EngineEvent::Status(status) => clock.print("STAT", describe_status(status)),
        EngineEvent::Error { message } => clock.print("ERROR", message),
    }
}

// ---------------------------------------------------------------------------------------------
// Running
// ---------------------------------------------------------------------------------------------

fn load_config(path: &PathBuf) -> Result<Config, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let (config, warnings) =
        m2k_core::store::parse(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    for warning in warnings {
        eprintln!("config warning: {warning}");
    }
    Ok(config)
}

fn sleep_until(target: Instant) {
    let now = Instant::now();
    if target > now {
        std::thread::sleep(target - now);
    }
}

pub fn run(args: &[String]) -> i32 {
    let mut positional = Vec::new();
    let mut print = PrintOptions::default();
    let mut repeats = false;
    let mut fail = Vec::new();
    let mut panic_key = None;
    let mut fail_up: HashMap<Key, u32> = HashMap::new();
    let mut backend_name: &'static str = "simulate";
    let mut text_chunk: Option<usize> = None;
    let mut settle_ms = 1000u64;
    let mut track_foreground = false;
    let mut panic_sink: Option<u64> = None;
    let mut absent: BTreeSet<String> = BTreeSet::new();
    let parse_key =
        |k: &String| serde_json::from_value::<Key>(serde_json::Value::String(k.clone())).ok();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--json" => print.json = true,
            "--clock" => print.clock = true,
            "--quiet-status" => print.quiet_status = true,
            "--repeats" => repeats = true,
            "--track-foreground" => track_foreground = true,
            "--panic-sink" => match iter.next().and_then(|v| v.parse().ok()) {
                Some(n) => panic_sink = Some(n),
                None => {
                    eprintln!("--panic-sink needs an event number");
                    return 2;
                }
            },
            "--fail" | "--panic" => match iter.next().and_then(parse_key) {
                Some(key) if arg == "--fail" => fail.push(key),
                Some(key) => panic_key = Some(key),
                None => {
                    eprintln!("{arg} needs a key code such as F13 or KeyA");
                    return 2;
                }
            },
            "--fail-up" => {
                let spec = iter.next().cloned().unwrap_or_default();
                let (key, count) = spec.split_once(':').unwrap_or((spec.as_str(), "1"));
                match (parse_key(&key.to_string()), count.parse::<u32>()) {
                    (Some(key), Ok(count)) => {
                        fail_up.insert(key, count);
                    }
                    _ => {
                        eprintln!("--fail-up needs a key code and an optional count, e.g. KeyA:3");
                        return 2;
                    }
                }
            }
            "--text-chunk" => match iter.next().and_then(|v| v.parse().ok()) {
                Some(chars) => text_chunk = Some(chars),
                None => {
                    eprintln!("--text-chunk needs a number of characters");
                    return 2;
                }
            },
            "--as-backend" => match iter.next() {
                Some(name) => backend_name = Box::leak(name.clone().into_boxed_str()),
                None => {
                    eprintln!("--as-backend needs a backend name");
                    return 2;
                }
            },
            "--absent-output" => match iter.next() {
                Some(name) => {
                    absent.insert(name.clone());
                }
                None => {
                    eprintln!("--absent-output needs a port name");
                    return 2;
                }
            },
            "--settle" => match iter.next().and_then(|v| v.parse().ok()) {
                Some(ms) => settle_ms = ms,
                None => {
                    eprintln!("--settle needs milliseconds");
                    return 2;
                }
            },
            "-h" | "--help" => {
                say(format_args!("{USAGE}"));
                return 0;
            }
            _ => positional.push(arg.clone()),
        }
    }
    let Some(config_path) = positional.first().map(PathBuf::from) else {
        eprintln!("{USAGE}");
        return 2;
    };
    let config = match load_config(&config_path) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    let script = match positional.get(1).map(String::as_str) {
        None | Some("-") => {
            let mut text = String::new();
            if let Err(error) = std::io::Read::read_to_string(&mut std::io::stdin(), &mut text) {
                eprintln!("cannot read the script from stdin: {error}");
                return 1;
            }
            text
        }
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) => {
                eprintln!("cannot read {path}: {error}");
                return 1;
            }
        },
    };
    let lines = match parse_script(&script) {
        Ok(lines) => lines,
        Err(error) => {
            eprintln!("{error}");
            return 2;
        }
    };

    let mappings: usize = config.profiles.iter().map(|p| p.mappings.len()).sum();
    say(format_args!(
        "simulating {} ({} profiles, {mappings} mappings), {} script lines{}",
        config_path.display(),
        config.profiles.len(),
        lines.len(),
        if repeats { ", backend repeats held keys" } else { "" }
    ));

    let clock = Arc::new(Clock(Instant::now()));
    let held = Arc::new(Mutex::new(Held::default()));
    let tally = Arc::new(Mutex::new(Tally::default()));
    let fail_up = Arc::new(Mutex::new(fail_up));
    let backend = |name: &'static str| LogInjector {
        name,
        clock: clock.clone(),
        held: held.clone(),
        repeats,
        fail: fail.clone(),
        panic: panic_key,
        fail_up: fail_up.clone(),
        text_chunk,
    };
    let injector = backend(backend_name);
    let midi_sent = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let absent = Arc::new(Mutex::new(absent));
    let disabled = Arc::new(Mutex::new(switched_off(&config)));
    let midi_out = LogMidiSink {
        clock: clock.clone(),
        absent: absent.clone(),
        disabled: disabled.clone(),
        sent: midi_sent.clone(),
    };
    let options = EngineOptions {
        injector: Box::new(injector),
        track_foreground,
        midi_out: Box::new(midi_out),
    };
    let engine = {
        let clock = clock.clone();
        let tally = tally.clone();
        Engine::start(config, options, {
            let seen = std::sync::atomic::AtomicU64::new(0);
            move |event| {
                let n = seen.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if panic_sink == Some(n) {
                    clock.print("SINK", format!("panicking on event {n} (--panic-sink)"));
                    panic!("simulated sink panic");
                }
                print_event(&clock, print, &tally, &event)
            }
        })
    };

    let mut parsers: HashMap<String, Parser> = HashMap::new();
    for line in &lines {
        sleep_until(clock.0 + Duration::from_millis(line.at_ms));
        if !matches!(line.step, Step::Wait) {
            clock.print(">", &line.text);
        }
        match &line.step {
            Step::Midi { device, bytes } => {
                let mut messages = Vec::new();
                parsers.entry(device.clone()).or_default().feed(bytes, &mut |m| messages.push(m));
                let time_us = clock.0.elapsed().as_micros() as u64;
                for message in messages {
                    engine.midi(device, time_us, message);
                }
            }
            Step::Foreground(app) => engine.set_foreground(app.clone()),
            Step::Paused(paused) => engine.set_paused(*paused),
            Step::Focused(focused) => engine.set_window_focused(*focused),
            Step::Learning(learning) => engine.set_learning(*learning),
            Step::Select(id) => engine.select_profile(id),
            Step::Disconnect(device) => {
                parsers.remove(device);
                engine.device_disconnected(device);
            }
            Step::ReleaseAll => engine.release_all(),
            Step::OutputGone(port) => {
                absent.lock().unwrap_or_else(|e| e.into_inner()).insert(port.clone());
            }
            Step::OutputBack(port) => {
                absent.lock().unwrap_or_else(|e| e.into_inner()).remove(port);
                engine.midi_outputs_changed(vec![port.clone()]);
            }
            Step::OutputsChanged(ports) => engine.midi_outputs_changed(ports.clone()),
            Step::Inputs(connected) => engine.midi_inputs_changed(connected.clone()),
            Step::Config(path) => match load_config(path) {
                Ok(config) => {
                    *disabled.lock().unwrap_or_else(|e| e.into_inner()) = switched_off(&config);
                    engine.set_config(config)
                }
                Err(error) => clock.print("ERROR", error),
            },
            Step::Run { delay_ms, action } => engine.run_action((**action).clone(), *delay_ms),
            Step::SwapBackend => engine.set_injector(Box::new(backend("simulate-2"))),
            Step::Status => {
                // Commands are asynchronous; give the engine a moment to catch up.
                std::thread::sleep(Duration::from_millis(5));
                clock.print("STAT", format!("(snapshot) {}", describe_status(&engine.status())));
            }
            Step::Echo | Step::Wait => {}
        }
    }

    std::thread::sleep(Duration::from_millis(settle_ms));
    clock.print("END", "stopping the engine");
    engine.stop();

    let held = held.lock().unwrap_or_else(|e| e.into_inner());
    let tally = tally.lock().unwrap_or_else(|e| e.into_inner());
    say(format_args!(
        "summary: {} key downs, {} key ups; fired: {} sent, {} skipped, {} failed; {} errors",
        held.key_downs, held.key_ups, tally.sent, tally.skipped, tally.failed, tally.errors
    ));
    let midi_sent = midi_sent.load(std::sync::atomic::Ordering::Relaxed);
    if midi_sent > 0 {
        say(format_args!("midi out: {midi_sent} messages"));
    }
    if held.keys.is_empty() && held.buttons.is_empty() {
        say(format_args!("RESULT: OK, nothing is left held"));
        0
    } else {
        say(format_args!("RESULT: STUCK! keys {:?} buttons {:?}", held.keys, held.buttons));
        3
    }
}
