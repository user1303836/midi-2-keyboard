//! `Run` and `Open` actions. Starting a process can take a while, so it happens on a helper
//! thread (started on first use); the process is detached and never waited for. Failures are
//! posted back to the engine and reported as `Outcome::Failed`.

use std::ffi::OsString;
use std::process::{Child, Command as Process, Stdio};
use std::thread;

use crossbeam_channel::{Sender, unbounded};

use super::compile::Launch;
use super::cx::OwnerInfo;
use super::worker::{Command, Envelope};

struct Job {
    launch: Launch,
    info: OwnerInfo,
    description: String,
}

pub struct Launcher {
    engine: Sender<Envelope>,
    jobs: Option<Sender<Job>>,
}

impl Launcher {
    pub fn new(engine: Sender<Envelope>) -> Self {
        Launcher { engine, jobs: None }
    }

    pub fn launch(&mut self, launch: Launch, info: OwnerInfo, description: String) {
        if self.jobs.is_none() {
            self.jobs = self.spawn_thread();
        }
        let job = Job { launch, info, description };
        let failed = match &self.jobs {
            Some(jobs) => jobs.send(job).err().map(|e| e.0),
            None => Some(job),
        };
        if let Some(job) = failed {
            let _ = self.engine.send(Envelope::now(Command::LaunchFailed {
                info: job.info,
                description: job.description,
                error: "the launcher thread is not running".into(),
            }));
        }
    }

    fn spawn_thread(&self) -> Option<Sender<Job>> {
        let (jobs, receiver) = unbounded::<Job>();
        let engine = self.engine.clone();
        let spawned = thread::Builder::new().name("m2k-launcher".into()).spawn(move || {
            init_thread();
            for job in receiver {
                if let Err(error) = start(&job.launch) {
                    let failed = Command::LaunchFailed {
                        info: job.info,
                        description: job.description,
                        error,
                    };
                    if engine.send(Envelope::now(failed)).is_err() {
                        return;
                    }
                }
            }
        });
        match spawned {
            Ok(_) => Some(jobs),
            Err(error) => {
                log::warn!("could not start the launcher thread: {error}");
                None
            }
        }
    }
}

fn start(launch: &Launch) -> Result<(), String> {
    match launch {
        Launch::Run { command, args, working_dir } => run(command, args, working_dir),
        Launch::Open { target } => open(target.trim()),
    }
}

fn run(command: &str, args: &[String], working_dir: &str) -> Result<(), String> {
    let command = command.trim();
    if command.is_empty() {
        return Err("no program to run".into());
    }
    let mut process = Process::new(resolve(command));
    process.args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    if !working_dir.trim().is_empty() {
        process.current_dir(working_dir.trim());
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        process.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        process.process_group(0);
    }
    let child = process.spawn().map_err(|error| format!("could not start {command}: {error}"))?;
    reap(child);
    Ok(())
}

/// Waits for the child on a throwaway thread so it does not linger as a zombie (Unix).
#[cfg(unix)]
fn reap(mut child: Child) {
    let _ = thread::Builder::new().name("m2k-reaper".into()).spawn(move || {
        let _ = child.wait();
    });
}

#[cfg(not(unix))]
fn reap(_child: Child) {}

/// Windows: `PATH` lookup with `PATHEXT` for bare names, so `code` finds `code.cmd` (the
/// standard library only tries `.exe`).
#[cfg(windows)]
fn resolve(command: &str) -> OsString {
    use std::path::Path;
    let path = Path::new(command);
    if path.components().count() > 1 || path.extension().is_some() {
        return command.into();
    }
    let extensions = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            for extension in extensions.split(';').filter(|e| !e.is_empty()) {
                let candidate = dir.join(format!("{command}{extension}"));
                if candidate.is_file() {
                    return candidate.into_os_string();
                }
            }
        }
    }
    command.into()
}

#[cfg(not(windows))]
fn resolve(command: &str) -> OsString {
    command.into()
}

#[cfg(windows)]
fn init_thread() {
    use windows_sys::Win32::System::Com::{
        COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoInitializeEx,
    };
    // ShellExecute may use COM (shell extensions); initialize this thread once.
    // SAFETY: called once on this thread, reserved pointer must be null.
    unsafe {
        CoInitializeEx(
            std::ptr::null(),
            (COINIT_APARTMENTTHREADED | COINIT_DISABLE_OLE1DDE) as u32,
        );
    }
}

#[cfg(not(windows))]
fn init_thread() {}

#[cfg(windows)]
fn open(target: &str) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    if target.is_empty() {
        return Err("nothing to open".into());
    }
    let wide =
        |s: &str| -> Vec<u16> { std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect() };
    let verb = wide("open");
    let file = wide(target);
    // SAFETY: NUL-terminated UTF-16 strings that outlive the call; null window, parameters and
    // directory are allowed.
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    let code = result as usize;
    if code > 32 { Ok(()) } else { Err(format!("could not open {target} (error {code})")) }
}

#[cfg(not(windows))]
fn open(target: &str) -> Result<(), String> {
    if target.is_empty() {
        return Err("nothing to open".into());
    }
    let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
    let child = Process::new(opener)
        .arg(target)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("could not run {opener}: {error}"))?;
    reap(child);
    Ok(())
}
