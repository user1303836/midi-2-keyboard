//! `m2k` — headless tools for midi-2-keyboard.

use std::path::PathBuf;

use ts_rs::TS;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("export-bindings") => {
            let dir = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("src/bindings"));
            export_bindings(&dir);
        }
        _ => {
            eprintln!("usage: m2k <command>\n\ncommands:\n  export-bindings [dir]   write the TypeScript bindings (default: src/bindings)");
            std::process::exit(2);
        }
    }
}

/// Writes the TypeScript declarations of every type shared with the UI.
fn export_bindings(dir: &PathBuf) {
    let cfg = ts_rs::Config::new().with_out_dir(dir).with_large_int("number");
    let result = (|| -> Result<(), ts_rs::ExportError> {
        m2k_core::Config::export_all(&cfg)?;
        m2k_core::EngineEvent::export_all(&cfg)?;
        m2k_core::EngineStatus::export_all(&cfg)?;
        m2k_midi::input::PortStatus::export_all(&cfg)?;
        m2k_output::platform::Permissions::export_all(&cfg)?;
        m2k_output::platform::ForegroundApp::export_all(&cfg)?;
        Ok(())
    })();
    match result {
        Ok(()) => println!("bindings written to {}", dir.display()),
        Err(error) => {
            eprintln!("failed to export bindings: {error}");
            std::process::exit(1);
        }
    }
}
