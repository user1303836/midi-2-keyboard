//! The engine thread. It owns the compiled config, the runtime state of every mapping and the
//! shared context ([`Cx`]); it turns commands and timers into behavior edges, keeps profiles,
//! liveness and the status snapshot up to date, and makes sure nothing stays held.
//!
//! Loop: wait for a command until the earliest timer deadline; fire the timers that were due
//! before the command arrived; handle the command; fire what is due now; deliver events. Each
//! iteration runs inside `catch_unwind`: a panic releases everything, resets the volatile state,
//! reports an error and the engine keeps running.
//!
//! The MIDI side (message -> observations -> behaviors, device bookkeeping) is in `dispatch`.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use m2k_midi::MidiMessage;
use m2k_output::platform::ForegroundApp;
use m2k_output::{Injector, Key};

use super::EngineOptions;
use super::behavior::{self, MappingRt};
use super::compile::{self, CAction, CompiledConfig, SourceIndex};
use super::cx::{Ctx, Cx, Gate, OwnerInfo, Request, TimerKind};
use super::executor::Executor;
use super::foreground::Tracker;
use super::launch::Launcher;
use super::midi_out::MidiSink;
use super::output::Output;
use super::pattern::Foreground;
use super::sys::TimerResolution;
use super::timers::TimerId;
use crate::events::{EngineEvent, EngineStatus, LatencyStats, Outcome};
use crate::model::{Action, Config, EngineOp, ProfileTarget};

mod dispatch;
mod feedback;

/// A command with the time it was posted (MIDI timing is measured from arrival, not from when
/// the engine got to it).
pub struct Envelope {
    pub at: Instant,
    pub command: Command,
}

impl Envelope {
    pub fn now(command: Command) -> Self {
        Envelope { at: Instant::now(), command }
    }
}

pub enum Command {
    Midi {
        device: String,
        message: MidiMessage,
    },
    SetConfig(Box<Config>),
    SetInjector(Box<dyn Injector>),
    SetMidiSink(Box<dyn MidiSink>),
    SetMonitoring(bool),
    MidiOutputsChanged(Vec<String>),
    MidiInputsChanged(Vec<String>),
    DeviceDisconnected(String),
    SetPaused(bool),
    SetWindowFocused(bool),
    SetLearning(bool),
    SelectProfile(String),
    SetForeground(Option<ForegroundApp>),
    TrackedForeground {
        generation: u64,
        app: Option<ForegroundApp>,
    },
    RunAction {
        action: Box<Action>,
        delay_ms: u32,
    },
    ReleaseAll,
    /// The launcher started (or could not start) a `Run` / `Open` action.
    Launched {
        info: OwnerInfo,
        description: String,
        error: Option<String>,
    },
    Shutdown,
}

/// State shared with the [`super::Engine`] handle.
#[derive(Default)]
pub struct Shared {
    pub status: Mutex<EngineStatus>,
}

/// Status events are coalesced to at most one per this interval (the final state is always
/// delivered).
const STATUS_INTERVAL: Duration = Duration::from_millis(50);

/// How often releases the backend refused are retried (see `Output`'s stuck keys).
const STUCK_RETRY_INTERVAL: Duration = Duration::from_millis(250);

#[derive(Default)]
struct Profiles {
    /// The profile the user selected (resolved).
    selected: Option<usize>,
    /// The first non-always-on profile whose app rules match the foreground application.
    candidate: Option<usize>,
    /// A manual selection overrides the app rule until the matching rule changes.
    manual: bool,
    effective: Option<usize>,
    auto_switched: bool,
}

/// The current status lives in [`Shared::status`], kept up to date in place.
#[derive(Default)]
struct StatusState {
    /// What the sink last received (`None` before the first status).
    emitted: Option<EngineStatus>,
    last_emit: Option<Instant>,
    pending: bool,
    timer: Option<TimerId>,
    /// Reused for the held keys.
    held: Vec<Key>,
}

pub struct Worker {
    sink: Box<dyn Fn(EngineEvent) + Send>,
    shared: Arc<Shared>,
    tx: Sender<Envelope>,
    track_foreground: bool,
    tracker: Option<Tracker>,
    tracker_generation: u64,
    cfg: CompiledConfig,
    maps: Vec<MappingRt>,
    by_key: HashMap<u64, usize>,
    next_key: u64,
    index: Arc<SourceIndex>,
    profiles: Profiles,
    status: StatusState,
    tests: HashMap<u64, CAction>,
    /// Pending retry of stuck keys / buttons.
    retry_timer: Option<TimerId>,
    /// Published latency statistics, and the timer for their next update.
    latency: Option<LatencyStats>,
    latency_timer: Option<TimerId>,
    next_test: u64,
    test_info: OwnerInfo,
    /// Mappings with feedback (indices).
    feedback: Vec<usize>,
    /// A feedback pass is needed even if nothing else changed.
    fb_due: bool,
    /// Off on the way out: nothing is sent on exit.
    fb_enabled: bool,
    /// Outputs reported missing (once each, until they appear or the config changes).
    fb_missing: HashSet<Arc<str>>,
    /// Last `off`s owed by mappings that went away.
    fb_final: Vec<feedback::FinalOff>,
    /// Bumped whenever outputs or feedback targets may have changed.
    outputs_generation: u64,
    /// Open MIDI inputs, as the runtime reports them (where feedback may answer).
    inputs: feedback::Inputs,
    cx: Cx,
}

/// The engine thread's body.
pub fn run(
    config: Config,
    options: EngineOptions,
    sink: Box<dyn Fn(EngineEvent) + Send>,
    shared: Arc<Shared>,
    tx: Sender<Envelope>,
    rx: Receiver<Envelope>,
) {
    let _resolution = TimerResolution::begin();
    super::sys::raise_thread_priority();
    let mut worker = Worker::new(config, options, sink, shared, tx);
    loop {
        let received = match worker.cx.timers.next_deadline() {
            Some(deadline) => rx.recv_deadline(deadline),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match catch_unwind(AssertUnwindSafe(|| worker.iteration(received))) {
            Ok(true) => {}
            Ok(false) => break,
            Err(panic) => worker.recover(panic),
        }
    }
    worker.stop();
}

impl Worker {
    pub(super) fn new(
        config: Config,
        options: EngineOptions,
        sink: Box<dyn Fn(EngineEvent) + Send>,
        shared: Arc<Shared>,
        tx: Sender<Envelope>,
    ) -> Self {
        let start = Instant::now();
        let empty = Config {
            version: config.version,
            settings: config.settings.clone(),
            devices: Vec::new(),
            profiles: Vec::new(),
            selected_profile: None,
        };
        let cfg = compile::compile(&empty);
        let out = Output::new(options.injector);
        let launcher = Launcher::new(tx.clone());
        let cx = Cx::new(start, cfg.settings.clone(), out, options.midi_out, launcher);
        let index = Arc::new(SourceIndex::build(&[], |_| false));
        let mut worker = Worker {
            sink,
            shared,
            tx,
            track_foreground: options.track_foreground,
            tracker: None,
            tracker_generation: 0,
            cfg,
            maps: Vec::new(),
            by_key: HashMap::new(),
            next_key: 0,
            index,
            profiles: Profiles::default(),
            status: StatusState::default(),
            tests: HashMap::new(),
            retry_timer: None,
            latency: None,
            latency_timer: None,
            next_test: 0,
            test_info: OwnerInfo::test(),
            feedback: Vec::new(),
            fb_due: false,
            fb_enabled: true,
            fb_missing: HashSet::new(),
            fb_final: Vec::new(),
            outputs_generation: 0,
            inputs: feedback::Inputs::default(),
            cx,
        };
        // The first install runs outside the loop's protection: a panic here must not take the
        // engine down for good (it would then silently never react).
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| worker.install(&config))) {
            worker.recover(panic);
        }
        worker.flush(true);
        worker
    }

    /// One envelope exactly as the loop handles it (for [`super::BenchEngine`]).
    pub(super) fn step(&mut self, envelope: Envelope) {
        if let Err(panic) = catch_unwind(AssertUnwindSafe(|| self.iteration(Ok(envelope)))) {
            self.recover(panic);
        }
    }

    /// The latest status snapshot.
    pub(super) fn shared(&self) -> &Arc<Shared> {
        &self.shared
    }

    /// One loop iteration. `false` = stop.
    fn iteration(&mut self, received: Result<Envelope, RecvTimeoutError>) -> bool {
        match received {
            Ok(envelope) => {
                let at = envelope.at.max(self.cx.now);
                self.run_timers(at);
                self.cx.now = at;
                if matches!(envelope.command, Command::Midi { .. }) {
                    // Measured from when `Engine::midi` was called, on the caller's thread.
                    self.cx.out.latency_origin = Some(envelope.at);
                }
                let keep_running = self.handle(envelope.command);
                self.cx.out.latency_origin = None;
                if !keep_running {
                    return false;
                }
                self.apply_requests();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return false,
        }
        self.run_timers(Instant::now());
        self.flush(false);
        true
    }

    fn run_timers(&mut self, until: Instant) {
        while let Some((id, kind)) = self.cx.timers.pop_due(until) {
            self.cx.now = self.cx.now.max(id.at());
            self.on_timer(id, kind);
            self.apply_requests();
        }
    }

    fn handle(&mut self, command: Command) -> bool {
        match command {
            Command::Midi { device, message } => self.on_midi(device, message),
            Command::SetConfig(config) => self.install(&config),
            Command::SetInjector(injector) => self.set_injector(injector),
            Command::SetMidiSink(sink) => {
                self.cx.midi = sink;
                self.outputs_generation += 1;
                self.fb_missing.clear();
                self.refresh_feedback();
            }
            Command::MidiOutputsChanged(ports) => self.midi_outputs_changed(ports),
            Command::SetMonitoring(on) => self.cx.monitor = on,
            Command::MidiInputsChanged(connected) => {
                self.inputs = feedback::Inputs {
                    connected: connected.into_iter().map(Arc::from).collect(),
                    known: true,
                };
                self.outputs_generation += 1;
                self.refresh_feedback();
            }
            Command::DeviceDisconnected(device) => self.device_disconnected(&device),
            Command::SetPaused(paused) => self.set_gate(|g| g.paused = paused),
            Command::SetWindowFocused(focused) => self.set_gate(|g| g.window_focused = focused),
            Command::SetLearning(learning) => self.set_gate(|g| g.learning = learning),
            Command::SelectProfile(id) => match self.cfg.profile_index(&id) {
                Some(index) => self.select(index),
                None => log::warn!("select_profile: there is no profile with id {id:?}"),
            },
            Command::SetForeground(app) => self.set_foreground(app),
            Command::TrackedForeground { generation, app } => {
                if generation == self.tracker_generation && self.tracker.is_some() {
                    self.set_foreground(app);
                }
            }
            Command::RunAction { action, delay_ms } => self.run_action(&action, delay_ms),
            Command::ReleaseAll => self.release_outputs(true),
            Command::Launched { info, description, error } => {
                let outcome = match error {
                    None => Outcome::Sent,
                    Some(error) => Outcome::Failed { error },
                };
                self.cx.report(&info, description, outcome);
            }
            Command::Shutdown => return false,
        }
        true
    }

    fn on_timer(&mut self, id: TimerId, kind: TimerKind) {
        match kind {
            TimerKind::Repeat(key)
            | TimerKind::TapHold(key)
            | TimerKind::MultiTap(key)
            | TimerKind::LongPress(key)
            | TimerKind::Debounce(key, _)
            | TimerKind::Limiter(key, _) => {
                if let Some(&i) = self.by_key.get(&key) {
                    behavior::on_timer(
                        &mut self.maps[i],
                        &self.cfg.mappings[i],
                        &mut self.cx,
                        kind,
                        id,
                    );
                }
            }
            TimerKind::Hold(owner) => self.cx.on_hold_timer(owner, id),
            TimerKind::Queue(owner) => self.cx.on_queue_timer(owner, id),
            TimerKind::Typematic => self.cx.on_typematic_timer(id),
            TimerKind::RunAction(test) => {
                if let Some(action) = self.tests.remove(&test) {
                    self.run_test(&action);
                }
            }
            TimerKind::Status => {
                if self.status.timer == Some(id) {
                    self.status.timer = None;
                }
            }
            TimerKind::Sensing(dev) => self.on_sensing_timer(dev, id),
            TimerKind::Latency => {
                if self.latency_timer == Some(id) {
                    self.latency_timer = None;
                }
            }
            TimerKind::RetryStuck => {
                if self.retry_timer == Some(id) {
                    self.retry_timer = None;
                    self.cx.out.retry_stuck();
                }
            }
        }
    }

    fn apply_requests(&mut self) {
        while !self.cx.requests.is_empty() {
            for request in std::mem::take(&mut self.cx.requests) {
                match request {
                    Request::Engine(EngineOp::Pause) => self.set_gate(|g| g.paused = true),
                    Request::Engine(EngineOp::Resume) => self.set_gate(|g| g.paused = false),
                    Request::Engine(EngineOp::TogglePause) => {
                        let paused = !self.cx.gate.paused;
                        self.set_gate(|g| g.paused = paused);
                    }
                    Request::Engine(EngineOp::ReleaseAll) => self.release_outputs(true),
                    Request::SwitchProfile(target, info, chosen) => {
                        self.switch_profile(&target, &info, chosen)
                    }
                }
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Configuration, profiles, liveness
    // -----------------------------------------------------------------------------------------

    /// Installs a config: mappings whose definition is unchanged keep their runtime state;
    /// everything held by changed or removed mappings is released.
    fn install(&mut self, config: &Config) {
        let cfg = compile::compile(config);
        // Reported once: an unrelated edit does not repeat a problem the user already saw.
        for error in cfg.errors.iter().filter(|e| !self.cfg.errors.contains(e)) {
            self.cx.error(error.clone());
        }

        let carry: Vec<Option<usize>> = {
            let old: HashMap<&str, usize> =
                self.cfg.mappings.iter().enumerate().map(|(i, m)| (&*m.id, i)).collect();
            cfg.mappings
                .iter()
                .map(|m| {
                    old.get(&*m.id).copied().filter(|&j| {
                        let previous = &self.cfg.mappings[j];
                        previous.profile_id == m.profile_id && same_behavior(&previous.def, &m.def)
                    })
                })
                .collect()
        };
        let mut kept = vec![false; self.maps.len()];
        for j in carry.iter().flatten() {
            kept[*j] = true;
        }
        // A light whose mapping goes away, or whose feedback changes, is switched off first.
        for (i, carried) in carry.iter().enumerate() {
            if let Some(j) = *carried
                && self.cfg.mappings[j].def.feedback != cfg.mappings[i].def.feedback
            {
                self.owe_final_off(j);
                self.maps[j].fb = Default::default();
            }
        }
        for (j, kept) in kept.iter().enumerate() {
            if !kept {
                self.owe_final_off(j);
                // Removed or changed: what it already fired stops as well.
                self.deactivate(j);
                let info = self.maps[j].info.clone();
                self.cx.remove_queue(info.owner);
                self.cx.release_owner(&info);
            }
        }

        let mut old: Vec<Option<MappingRt>> =
            std::mem::take(&mut self.maps).into_iter().map(Some).collect();
        let mut maps = Vec::with_capacity(cfg.mappings.len());
        for (def, carried) in cfg.mappings.iter().zip(&carry) {
            match carried.and_then(|j| old[j].take()) {
                Some(runtime) => maps.push(runtime),
                None => {
                    self.next_key += 1;
                    maps.push(MappingRt::new(self.next_key, def));
                }
            }
        }

        // `selected_profile` changed to something other than what the engine has selected: a
        // manual selection. Otherwise keep the current selection (a profile switch action may
        // have changed it since, and the app echoes it back in later configs).
        let current = self.profiles.selected.map(|i| self.cfg.profiles[i].id.to_string());
        let selected_now =
            cfg.selected_profile != self.cfg.selected_profile && cfg.selected_profile != current;
        let wanted = if selected_now { cfg.selected_profile.clone() } else { current };

        self.cfg = cfg;
        self.maps = maps;
        self.by_key = self.maps.iter().enumerate().map(|(i, m)| (m.key, i)).collect();
        self.cx.settings = self.cfg.settings.clone();
        self.cx.selectable =
            self.cfg.profiles.iter().filter(|p| !p.always_on).map(|p| p.name.clone()).collect();

        self.profiles.selected = resolve_selected(&self.cfg, wanted.as_deref());
        self.refresh_candidate();
        if selected_now {
            self.profiles.manual = self.profiles.candidate.is_some();
        }

        let suspend = self.cfg.settings.suspend_while_focused;
        self.set_gate(|g| g.suspend_while_focused = suspend);
        self.update_tracker();
        self.refresh_liveness(true);
        self.cx.dirty = true;

        self.feedback = (0..self.cfg.mappings.len())
            .filter(|&i| self.cfg.mappings[i].feedback.is_some())
            .collect();
        // New mappings (and ones whose feedback changed) have sent nothing yet, so the next pass
        // sends their state; unchanged ones are not resent (the editor saves on every change).
        self.outputs_generation += 1;
        self.fb_missing.clear();
        self.fb_due = true;
    }

    fn deactivate(&mut self, i: usize) {
        behavior::deactivate(&mut self.maps[i], &self.cfg.mappings[i], &mut self.cx);
    }

    fn effective_profile(&self) -> (Option<usize>, bool) {
        if self.cfg.settings.auto_switch_profiles
            && !self.profiles.manual
            && let Some(candidate) = self.profiles.candidate
        {
            return (Some(candidate), true);
        }
        (self.profiles.selected, false)
    }

    /// Recomputes which mappings are live; mappings that stop being live release everything.
    fn refresh_liveness(&mut self, rebuild_index: bool) {
        let (effective, auto_switched) = self.effective_profile();
        if self.profiles.effective != effective || self.profiles.auto_switched != auto_switched {
            self.profiles.effective = effective;
            self.profiles.auto_switched = auto_switched;
            self.cx.dirty = true;
        }
        let mut changed = rebuild_index;
        for i in 0..self.maps.len() {
            let profile = self.cfg.mappings[i].profile;
            let live = self.cfg.profiles[profile].always_on || Some(profile) == effective;
            if self.maps[i].live != live {
                changed = true;
                // Feedback: off for the mapping that leaves, the current state for one that comes.
                self.maps[i].fb.force = true;
                self.fb_due = true;
                if !live {
                    self.deactivate(i);
                }
                self.maps[i].live = live;
            }
        }
        if changed {
            // Feedback targets are resolved again for the new live set.
            self.outputs_generation += 1;
            let maps = &self.maps;
            self.index = Arc::new(SourceIndex::build(&self.cfg.mappings, |i| maps[i].live));
            self.cx.dirty = true;
        }
    }

    fn refresh_candidate(&mut self) {
        let candidate = match (&self.cx.foreground, self.cfg.settings.auto_switch_profiles) {
            (Some(app), true) => self
                .cfg
                .profiles
                .iter()
                .position(|p| !p.always_on && p.apps.iter().any(|rule| rule.matches(app))),
            _ => None,
        };
        if candidate != self.profiles.candidate {
            self.profiles.candidate = candidate;
            self.profiles.manual = false;
        }
    }

    fn select(&mut self, index: usize) {
        self.profiles.selected = Some(index);
        self.profiles.manual = self.profiles.candidate.is_some();
        self.cx.dirty = true;
        self.refresh_liveness(false);
    }

    fn switch_profile(&mut self, target: &ProfileTarget, info: &OwnerInfo, chosen: Option<usize>) {
        let index = match target {
            ProfileTarget::ByValue => {
                // `chosen` indexes the selectable profiles (resolved when the action fired).
                let selectable =
                    (0..self.cfg.profiles.len()).filter(|&i| !self.cfg.profiles[i].always_on);
                match chosen.and_then(|n| selectable.into_iter().nth(n)) {
                    Some(index) => index,
                    None => return,
                }
            }
            ProfileTarget::Profile { id } => match self.cfg.profile_index(id) {
                Some(index) => index,
                None => {
                    let error = format!("there is no profile with id \"{id}\"");
                    self.cx.report(
                        info,
                        format!("switch to profile \"{id}\""),
                        Outcome::Failed { error },
                    );
                    return;
                }
            },
            ProfileTarget::Next | ProfileTarget::Previous => {
                let selectable: Vec<usize> = (0..self.cfg.profiles.len())
                    .filter(|&i| !self.cfg.profiles[i].always_on)
                    .collect();
                if selectable.is_empty() {
                    return;
                }
                let current = self
                    .profiles
                    .effective
                    .filter(|i| selectable.contains(i))
                    .or(self.profiles.selected);
                let n = selectable.len();
                let next = match current.and_then(|c| selectable.iter().position(|&i| i == c)) {
                    Some(pos) if matches!(target, ProfileTarget::Next) => (pos + 1) % n,
                    Some(pos) => (pos + n - 1) % n,
                    None => 0,
                };
                selectable[next]
            }
        };
        self.select(index);
    }

    fn set_foreground(&mut self, app: Option<ForegroundApp>) {
        if self.cx.foreground.as_ref().map(|f| &f.app) == app.as_ref() {
            return;
        }
        self.cx.foreground = app.map(Foreground::new);
        self.cx.dirty = true;
        self.refresh_candidate();
        self.refresh_liveness(false);
    }

    fn update_tracker(&mut self) {
        let needed = self.track_foreground && self.cfg.needs_foreground;
        if needed && self.tracker.is_none() {
            self.tracker_generation += 1;
            self.tracker = Tracker::start(self.tx.clone(), self.tracker_generation);
        } else if !needed && self.tracker.is_some() {
            self.tracker = None;
            self.tracker_generation += 1;
            if self.cx.foreground.take().is_some() {
                self.cx.dirty = true;
                self.refresh_candidate();
            }
        }
    }

    // -----------------------------------------------------------------------------------------
    // Gating and releases
    // -----------------------------------------------------------------------------------------

    fn set_gate(&mut self, change: impl FnOnce(&mut Gate)) {
        let before = self.cx.gate;
        change(&mut self.cx.gate);
        let after = self.cx.gate;
        if !before.blocks_os() && after.blocks_os() {
            self.release_outputs(false);
        }
        if before.paused != after.paused
            || before.learning != after.learning
            || before.focus_suspended() != after.focus_suspended()
        {
            self.cx.dirty = true;
        }
    }

    /// Releases what is held: OS output (`all = false`) or everything including layer holds and
    /// layer toggles (`all = true`, the panic button). Running and queued actions are dropped.
    fn release_outputs(&mut self, all: bool) {
        for i in 0..self.maps.len() {
            behavior::release(&mut self.maps[i], &self.cfg.mappings[i], &mut self.cx, all);
        }
        self.cx.abort_all_queues();
        self.cx.release_all_holds();
        self.cx.typematic_cancel();
        // Safety net: nothing may survive, whoever was supposed to release it.
        for error in self.cx.out.release_everything() {
            self.cx.error(format!("Could not release a key or button: {error}"));
        }
        self.cx.dirty = true;
    }

    fn set_injector(&mut self, injector: Box<dyn Injector>) {
        self.release_outputs(false);
        drop(self.cx.out.replace_injector(injector));
        self.cx.dirty = true;
    }

    // -----------------------------------------------------------------------------------------
    // Actions run from the UI
    // -----------------------------------------------------------------------------------------

    fn run_action(&mut self, action: &Action, delay_ms: u32) {
        // An empty action is no action (the Test button of a slot left blank).
        if action.is_empty() {
            return;
        }
        let compiled = compile::action(action, &self.cfg.settings, &self.cfg.profiles);
        if delay_ms == 0 {
            self.run_test(&compiled);
            return;
        }
        self.next_test += 1;
        self.tests.insert(self.next_test, compiled);
        self.cx.after(delay_ms, TimerKind::RunAction(self.next_test));
    }

    fn run_test(&mut self, action: &CAction) {
        let info = self.test_info.clone();
        self.cx.execute(&info, action, Ctx::default(), false);
    }

    // -----------------------------------------------------------------------------------------
    // Events and status
    // -----------------------------------------------------------------------------------------

    fn emit(&self, event: EngineEvent) {
        let sink = &self.sink;
        if catch_unwind(AssertUnwindSafe(|| sink(event))).is_err() {
            log::error!("the engine event sink panicked; the event was dropped");
        }
    }

    /// Delivers queued events and (coalesced) status changes. `force` delivers the status now.
    fn flush(&mut self, force: bool) {
        // After this event's key output, before its events go out.
        self.feedback_pass();
        if !self.cx.outbox.is_empty() {
            let mut events = std::mem::take(&mut self.cx.outbox);
            for event in events.drain(..) {
                self.emit(event);
            }
            if self.cx.outbox.is_empty() {
                self.cx.outbox = events;
            }
        }
        // A release the backend refused: keep retrying until the OS accepts it.
        if self.cx.out.has_stuck() && self.retry_timer.is_none() {
            let at = self.cx.now.max(Instant::now()) + STUCK_RETRY_INTERVAL;
            self.retry_timer = Some(self.cx.timers.schedule(at, TimerKind::RetryStuck));
        }
        self.publish_latency();
        if self.cx.out.changed {
            self.cx.out.changed = false;
            self.cx.dirty = true;
        }
        if self.cx.dirty {
            self.cx.dirty = false;
            if self.update_status() {
                self.status.pending = true;
            }
        }
        if self.status.pending {
            let now = Instant::now();
            let due = self.status.last_emit.map_or(now, |at| at + STATUS_INTERVAL);
            if force || now >= due {
                self.status.pending = false;
                self.cx.timers.cancel_slot(&mut self.status.timer);
                // Changes that cancelled out within the interval (a tap) need no event.
                let current = self.shared.status.lock().unwrap_or_else(|e| e.into_inner()).clone();
                if self.status.emitted.as_ref() != Some(&current) {
                    self.status.last_emit = Some(now);
                    self.status.emitted = Some(current.clone());
                    self.emit(EngineEvent::Status(current));
                }
            } else if self.status.timer.is_none() {
                self.status.timer = Some(self.cx.timers.schedule(due, TimerKind::Status));
            }
        }
    }

    /// Latency statistics change the status at most once a second; one timer makes sure the
    /// last burst's numbers (and their expiry) are published without further traffic.
    fn publish_latency(&mut self) {
        let now = self.cx.now.max(Instant::now());
        if let Some(stats) = self.cx.out.meter.publish(now)
            && stats != self.latency
        {
            self.latency = stats;
            self.cx.dirty = true;
        }
        let due = self.cx.out.meter.next_due();
        if self.latency_timer.map(TimerId::at) != due {
            self.cx.timers.cancel_slot(&mut self.latency_timer);
            if let Some(at) = due {
                self.latency_timer = Some(self.cx.timers.schedule(at, TimerKind::Latency));
            }
        }
    }

    /// Brings the shared status snapshot up to date in place; `true` when something changed.
    /// Only what changed is allocated (every key press changes the held keys, so building a
    /// whole new status each time was most of what a tap cost).
    fn update_status(&mut self) -> bool {
        let mut held = std::mem::take(&mut self.status.held);
        self.cx.out.held_keys_into(&mut held);
        let mut guard = self.shared.status.lock().unwrap_or_else(|e| e.into_inner());
        let status = &mut *guard;
        let mut changed = false;
        let mut set = |field: &mut bool, value: bool| {
            if *field != value {
                *field = value;
                changed = true;
            }
        };
        set(&mut status.paused, self.cx.gate.paused);
        set(&mut status.focus_suspended, self.cx.gate.focus_suspended());
        set(&mut status.learning, self.cx.gate.learning);
        set(&mut status.auto_switched, self.profiles.auto_switched);
        let id = |index: Option<usize>| index.map(|i| &*self.cfg.profiles[i].id);
        for (field, index) in [
            (&mut status.selected_profile, self.profiles.selected),
            (&mut status.effective_profile, self.profiles.effective),
        ] {
            if field.as_deref() != id(index) {
                *field = id(index).map(str::to_string);
                changed = true;
            }
        }
        if !self.cx.layers.matches(&status.layers) {
            status.layers = self.cx.layers.names();
            changed = true;
        }
        if status.held_keys != held {
            std::mem::swap(&mut status.held_keys, &mut held);
            changed = true;
        }
        let engaged = || {
            self.maps
                .iter()
                .zip(&self.cfg.mappings)
                .filter(|(rt, _)| rt.engaged())
                .map(|(_, def)| &*def.id)
        };
        if !engaged().eq(status.engaged_mappings.iter().map(String::as_str)) {
            // Merged in place (both lists are in config order): only a newly engaged mapping
            // allocates its id.
            let list = &mut status.engaged_mappings;
            let mut at = 0;
            for id in engaged() {
                match list[at..].iter().position(|known| known == id) {
                    Some(0) => {}
                    // The ones before it are no longer engaged.
                    Some(skip) => drop(list.drain(at..at + skip)),
                    None => list.insert(at, id.to_string()),
                }
                at += 1;
            }
            list.truncate(at);
            changed = true;
        }
        let foreground = self.cx.foreground.as_ref().map(|f| &f.app);
        if status.foreground.as_ref() != foreground {
            status.foreground = foreground.cloned();
            changed = true;
        }
        if status.backend != self.cx.out.name() {
            status.backend = self.cx.out.name().to_string();
            changed = true;
        }
        if status.latency != self.latency {
            status.latency = self.latency;
            changed = true;
        }
        drop(guard);
        self.status.held = held;
        changed
    }

    /// Whether `Midi` / `Fired` events are made (for [`super::BenchEngine`]).
    pub(super) fn set_monitoring(&mut self, on: bool) {
        self.cx.monitor = on;
    }

    // -----------------------------------------------------------------------------------------
    // Failure handling and shutdown
    // -----------------------------------------------------------------------------------------

    fn recover(&mut self, panic: Box<dyn Any + Send>) {
        let message = panic
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| panic.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".into());
        log::error!("engine panic: {message}");
        // The command being handled is gone: its latency must not be pinned on what comes next.
        self.cx.out.latency_origin = None;
        // Release in an orderly way if the state still allows it, then unconditionally.
        let _ = catch_unwind(AssertUnwindSafe(|| self.release_outputs(true)));
        let _ = catch_unwind(AssertUnwindSafe(|| {
            let _ = self.cx.out.release_everything();
        }));
        let _ = catch_unwind(AssertUnwindSafe(|| self.reset_volatile()));
        self.cx.outbox.push(EngineEvent::Error {
            message: format!(
                "Internal engine error ({message}). Everything that was held has been released."
            ),
        });
        let _ = catch_unwind(AssertUnwindSafe(|| self.flush(true)));
    }

    /// Forgets all in-flight state (timers, queues, behavior state). Config, profiles, layers
    /// and gating survive.
    fn reset_volatile(&mut self) {
        self.cx.timers.clear();
        self.cx.exec = Executor::default();
        self.cx.typematic = None;
        self.cx.requests.clear();
        self.cx.devices.forget_timers();
        // Layer holds belong to mapping states, which are reset below (latched layers stay).
        self.cx.layers.release_held();
        // The panic may have struck in the middle of a config swap: re-derive everything that
        // is indexed by mapping position.
        if self.maps.len() != self.cfg.mappings.len() {
            self.maps.clear();
            for def in &self.cfg.mappings {
                self.next_key += 1;
                self.maps.push(MappingRt::new(self.next_key, def));
            }
        }
        for (rt, def) in self.maps.iter_mut().zip(&self.cfg.mappings) {
            rt.reset(def);
        }
        self.by_key = self.maps.iter().enumerate().map(|(i, m)| (m.key, i)).collect();
        let profiles = self.cfg.profiles.len();
        if self.profiles.selected.is_some_and(|i| i >= profiles) {
            self.profiles.selected = resolve_selected(&self.cfg, None);
        }
        if self.profiles.candidate.is_some_and(|i| i >= profiles) {
            self.profiles.candidate = None;
        }
        self.profiles.effective = None;
        self.refresh_liveness(true);
        // Mapping states were reset: lights that were on are switched off at the next pass.
        self.fb_due = true;
        self.tests.clear();
        self.retry_timer = None;
        self.latency_timer = None;
        self.status.timer = None;
        self.status.pending = true;
        self.cx.dirty = true;
    }

    /// Final cleanup on the engine thread: release everything and deliver the last status.
    pub(super) fn stop(&mut self) {
        // Lights keep showing the state the app was in.
        self.fb_enabled = false;
        let _ = catch_unwind(AssertUnwindSafe(|| {
            self.release_outputs(true);
            // Last chances for releases the backend refused (the thread is about to end, so a
            // short wait no longer delays anything).
            for _ in 0..3 {
                if !self.cx.out.retry_stuck() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            self.tracker = None;
            self.cx.timers.clear();
            self.retry_timer = None;
            self.flush(true);
        }));
    }
}

/// Whether two definitions of a mapping behave the same (its label does not matter). Destructured
/// exhaustively so a field added to `Mapping` later has to be considered here.
fn same_behavior(a: &crate::model::Mapping, b: &crate::model::Mapping) -> bool {
    // `feedback` alone does not reset the mapping: a new light colour keeps a latched toggle
    // latched (the install sends the old light's `off` and the new feedback's current state).
    let crate::model::Mapping {
        id,
        name: _,
        enabled,
        source,
        behavior,
        conditions,
        rate_limit,
        feedback: _,
    } = a;
    *id == b.id
        && *enabled == b.enabled
        && *source == b.source
        && *behavior == b.behavior
        && *conditions == b.conditions
        && *rate_limit == b.rate_limit
}

/// The selected profile: `wanted` if it exists, else the first non-always-on profile, else the
/// first profile.
fn resolve_selected(cfg: &CompiledConfig, wanted: Option<&str>) -> Option<usize> {
    wanted
        .and_then(|id| cfg.profile_index(id))
        .or_else(|| cfg.profiles.iter().position(|p| !p.always_on))
        .or(if cfg.profiles.is_empty() { None } else { Some(0) })
}
