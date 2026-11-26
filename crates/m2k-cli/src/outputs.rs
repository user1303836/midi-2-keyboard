//! `m2k outputs` and `m2k send` (owned by the MIDI layer).

use std::time::Duration;

use m2k_midi::input::{PortPolicy, PortState, PortStatus};
use m2k_midi::output::{OutputManager, OutputOptions, split_messages};

use crate::devices::{hex, parse_hex};

const OUTPUTS_USAGE: &str = "usage: m2k outputs [--json]

Lists the MIDI output ports. Nothing is opened: an output is only opened when something is sent
to it.

  --json   print the list as JSON (the PortStatus objects the app uses)";

const SEND_USAGE: &str = "usage: m2k send <port> <hex bytes>...

Sends MIDI bytes to one output port and exits once they are out. <port> is a name as `m2k
outputs` lists it, or a part of one that matches only that port (case-insensitive). The bytes
may hold several messages, running status and SysEx:

  m2k send wavetable B0 07 64           channel 1 volume to 100
  m2k send \"Launch\" 90 3C 7F 3C 00      note on + note off (running status)
  m2k send \"Launch\" F0 7E 7F 06 01 F7   SysEx (identity request)";

/// How long `m2k send` waits for the bytes to go out.
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

fn options() -> OutputOptions {
    OutputOptions { client_name: "m2k".into(), ..OutputOptions::default() }
}

pub fn outputs(args: &[String]) -> i32 {
    let mut json = false;
    for arg in args {
        match arg.as_str() {
            "--json" => json = true,
            "-h" | "--help" => {
                println!("{OUTPUTS_USAGE}");
                return 0;
            }
            other => {
                eprintln!("m2k outputs: unexpected argument {other:?}\n\n{OUTPUTS_USAGE}");
                return 2;
            }
        }
    }
    let manager = OutputManager::start(options(), PortPolicy::All, |_| {});
    let ports = manager.ports();
    drop(manager);

    if json {
        return match serde_json::to_string_pretty(&ports) {
            Ok(text) => {
                println!("{text}");
                0
            }
            Err(error) => {
                eprintln!("m2k outputs: {error}");
                1
            }
        };
    }
    if ports.is_empty() {
        println!("No MIDI output ports found.");
        return 0;
    }
    let width = ports.iter().map(|p| p.name.chars().count()).max().unwrap_or(0);
    let plural = if ports.len() == 1 { "" } else { "s" };
    println!("{} MIDI output port{plural}:", ports.len());
    for port in &ports {
        println!("  {:<width$}  {}", port.name, describe_output_state(port));
    }
    0
}

pub fn send(args: &[String]) -> i32 {
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{SEND_USAGE}");
        return 0;
    }
    let Some((wanted, hex_args)) = args.split_first() else {
        eprintln!("m2k send: missing port\n\n{SEND_USAGE}");
        return 2;
    };
    let bytes = match parse_hex(&hex_args.join(" ")) {
        Ok(bytes) => bytes,
        Err(error) => {
            eprintln!("m2k send: {error}\n\n{SEND_USAGE}");
            return 2;
        }
    };
    let messages = match split_messages(&bytes) {
        Ok(messages) => messages,
        Err(error) => {
            eprintln!("m2k send: {error}");
            return 2;
        }
    };

    let manager = OutputManager::start(options(), PortPolicy::All, |_| {});
    let ports = manager.ports();
    let name = match resolve_port(&ports, wanted) {
        Ok(name) => name,
        Err(error) => {
            eprintln!("m2k send: {error}");
            return 1;
        }
    };
    if let Err(error) = manager.send(Some(&name), &bytes) {
        eprintln!("m2k send: {error}");
        return 1;
    }
    let flushed = manager.flush(SEND_TIMEOUT);
    let port = manager.ports().into_iter().find(|p| p.name == name);
    let dropped = manager.dropped();
    drop(manager);

    match port {
        Some(port) if flushed && port.state == PortState::Connected && dropped == 0 => {
            let plural = if messages.len() == 1 { "" } else { "s" };
            println!("Sent {} message{plural} to {name}:", messages.len());
            for message in &messages {
                println!("  {}", hex(message));
            }
            0
        }
        Some(port) if port.state == PortState::Error => {
            eprintln!("m2k send: {name}: {}", port.error);
            1
        }
        Some(_) if !flushed => {
            eprintln!("m2k send: {name} did not take the bytes within {SEND_TIMEOUT:?}");
            1
        }
        Some(_) => {
            eprintln!("m2k send: the bytes for {name} were dropped");
            1
        }
        None => {
            eprintln!("m2k send: {name} went away before the bytes were sent");
            1
        }
    }
}

/// The port called `wanted`, or the only one whose name contains it (case-insensitive).
fn resolve_port(ports: &[PortStatus], wanted: &str) -> Result<String, String> {
    if let Some(port) = ports.iter().find(|p| p.name == wanted) {
        return Ok(port.name.clone());
    }
    let needle = wanted.to_lowercase();
    let matches: Vec<&PortStatus> =
        ports.iter().filter(|p| p.name.to_lowercase().contains(&needle)).collect();
    match matches.as_slice() {
        [port] => Ok(port.name.clone()),
        [] => {
            let names: Vec<&str> = ports.iter().map(|p| p.name.as_str()).collect();
            Err(format!(
                "no MIDI output matches {wanted:?} (present: {})",
                if names.is_empty() { "none".into() } else { names.join(", ") }
            ))
        }
        several => {
            let names: Vec<&str> = several.iter().map(|p| p.name.as_str()).collect();
            Err(format!("{wanted:?} matches several outputs: {}", names.join(", ")))
        }
    }
}

fn describe_output_state(port: &PortStatus) -> String {
    match port.state {
        PortState::Connected => "open".to_owned(),
        PortState::Disabled => "present (opened on first send)".to_owned(),
        PortState::Error => format!("error: {}", port.error),
    }
}
