//! `m2k` — headless tools for midi-2-keyboard.
//!
//! Each subcommand lives in its own module so several people can work here without conflicts.

mod bindings;
mod devices;
mod outputs;
mod run;
mod simulate;

const USAGE: &str = "usage: m2k <command> [args]

commands:
  devices                  list MIDI input ports
  monitor [port-filter]    print incoming MIDI messages
  outputs                  list MIDI output ports
  send <port> <hex>...     send MIDI bytes to an output port
  run [--config <path>] [--dry-run]
                           run a config headless against the real MIDI inputs (Ctrl+C stops)
  simulate <args>          run a config against scripted MIDI input and print what would be sent
  export-bindings [dir]    write the TypeScript bindings (default: src/bindings)";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let code = match args.first().map(String::as_str) {
        Some("devices") => devices::devices(rest),
        Some("monitor") => devices::monitor(rest),
        Some("outputs") => outputs::outputs(rest),
        Some("send") => outputs::send(rest),
        Some("run") => run::run(rest),
        Some("simulate") => simulate::run(rest),
        Some("export-bindings") => bindings::run(rest),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}
