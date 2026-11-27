use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows")
        && env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("gnu")
    {
        keep_only_tauris_manifest();
    }
    tauri_build::build()
}

/// GNU toolchain only: MinGW's GCC adds its own `default-manifest.o` to every program. The WinLibs
/// build tags that manifest en-US instead of language-neutral, so ld cannot tell it is only a
/// default: it keeps it next to Tauri's ("multiple non-default manifests") and Windows happens to
/// use Tauri's, the first. GCC looks for the file in `-B` directories before its own, so an empty
/// stand-in there leaves Tauri's manifest (Common Controls v6, which the message dialogs'
/// `TaskDialogIndirect` needs) as the only one, as in an MSVC build.
fn keep_only_tauris_manifest() {
    let dir =
        PathBuf::from(env::var("OUT_DIR").expect("OUT_DIR is set")).join("no-default-manifest");
    fs::create_dir_all(&dir).expect("create the manifest stand-in's directory");
    // An empty archive: the linker accepts it and takes nothing from it.
    fs::write(dir.join("default-manifest.o"), b"!<arch>\n").expect("write the manifest stand-in");
    // GCC prepends -B to file names: forward slashes, and one at the end.
    let prefix = dir.to_string_lossy().replace('\\', "/");
    println!("cargo:rustc-link-arg-bins=-B{prefix}/");
}
