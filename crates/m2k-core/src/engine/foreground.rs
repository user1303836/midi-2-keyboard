//! Foreground application tracking: while some profile rule or condition needs it, a helper
//! thread polls the platform about four times per second and posts changes to the engine.
//! Dropping the [`Tracker`] stops the thread at its next poll.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread;
use std::time::Duration;

use crossbeam_channel::{RecvTimeoutError, Sender, bounded};
use m2k_output::platform;

use super::worker::{Command, Envelope};

const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub struct Tracker {
    _stop: Sender<()>,
}

impl Tracker {
    /// Starts polling. `generation` tags the reports so the engine can ignore those of a
    /// tracker it already stopped.
    pub fn start(engine: Sender<Envelope>, generation: u64) -> Option<Tracker> {
        let (stop, stopped) = bounded::<()>(0);
        let spawned = thread::Builder::new().name("m2k-foreground".into()).spawn(move || {
            let mut last = None;
            let mut first = true;
            loop {
                let app = catch_unwind(AssertUnwindSafe(platform::foreground_app)).unwrap_or(None);
                if first || app != last {
                    first = false;
                    let report = Command::TrackedForeground { generation, app: app.clone() };
                    if engine.send(Envelope::now(report)).is_err() {
                        return;
                    }
                    last = app;
                }
                match stopped.recv_timeout(POLL_INTERVAL) {
                    Err(RecvTimeoutError::Timeout) => {}
                    _ => return,
                }
            }
        });
        match spawned {
            Ok(_) => Some(Tracker { _stop: stop }),
            Err(error) => {
                log::warn!("could not start the foreground tracker: {error}");
                None
            }
        }
    }
}
