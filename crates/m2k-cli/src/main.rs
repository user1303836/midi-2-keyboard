//! `m2k` — headless tools for midi-2-keyboard.
//!
//! Each subcommand lives in its own module so several people can work here without conflicts.

mod bench;
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
  bench [options]          measure the engine's hot path (throughput, cost per message, allocations)
  export-bindings [dir]    write the TypeScript bindings (default: src/bindings)";

/// Counts allocations while `m2k bench` measures; otherwise just the system allocator.
#[global_allocator]
static ALLOCATOR: bench::CountingAllocator = bench::CountingAllocator;

fn main() {
    // `m2k run` presses real keys: a panic must not leave one of them down.
    m2k_output::install_panic_release(m2k_midi::input::catching_panics);
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or(&[]);
    let code = match args.first().map(String::as_str) {
        Some("devices") => devices::devices(rest),
        Some("monitor") => devices::monitor(rest),
        Some("outputs") => outputs::outputs(rest),
        Some("send") => outputs::send(rest),
        Some("run") => run::run(rest),
        Some("simulate") => simulate::run(rest),
        Some("bench") => bench::run(rest),
        Some("export-bindings") => bindings::run(rest),
        _ => {
            eprintln!("{USAGE}");
            2
        }
    };
    std::process::exit(code);
}
