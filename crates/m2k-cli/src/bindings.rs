//! `m2k export-bindings [dir]` — writes the TypeScript declarations of every type shared with
//! the UI. Add new root types here when they are not reachable from the ones listed.

use std::path::PathBuf;

use ts_rs::TS;

pub fn run(args: &[String]) -> i32 {
    let dir = PathBuf::from(args.first().map(String::as_str).unwrap_or("src/bindings"));
    let cfg = ts_rs::Config::new().with_out_dir(&dir).with_large_int("number");
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
        Ok(()) => {
            println!("bindings written to {}", dir.display());
            0
        }
        Err(error) => {
            eprintln!("failed to export bindings: {error}");
            1
        }
    }
}
