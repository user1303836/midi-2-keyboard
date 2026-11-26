//! The engine thread. It owns the compiled config, the runtime state of every mapping and the
//! shared context ([`Cx`]); it turns commands and timers into behavior edges, keeps profiles,
//! liveness and the status snapshot up to date, and makes sure nothing stays held.
//!
//! Loop: wait for a command until the earliest timer deadline; fire the timers that were due
//! before the command arrived; handle the command; fire what is due now; deliver events. Each
//! iteration runs inside `catch_unwind`: a panic releases everything, resets the volatile state,
//! reports an error and the engine keeps running.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use m2k_midi::{Derived, MidiMessage, ParameterChange, ParameterKind};
use m2k_output::Injector;
use m2k_output::platform::ForegroundApp;

use super::EngineOptions;
use super::behavior::{self, MappingRt};
use super::compile::{self, CAction, CSourceKind, CompiledConfig, SourceIndex, system_index};
use super::cx::{Ctx, Cx, Gate, OwnerInfo, Request, TimerKind};
use super::executor::Executor;
use super::foreground::Tracker;
use super::launch::Launcher;
use super::output::Output;
use super::pattern::Foreground;
use super::source::{DeviceId, InstKey, Obs, SENSING_TIMEOUT, relative_ticks};
use super::sys::TimerResolution;
use super::timers::TimerId;
use crate::events::{EngineEvent, EngineStatus, MidiLog, Outcome};
use crate::model::{
    Action, Config, EngineOp, ProfileTarget, SystemMessage, ValueEncoding, ValueRange,
};

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
    Midi { device: String, message: MidiMessage },
    SetConfig(Box<Config>),
    SetInjector(Box<dyn Injector>),
    DeviceDisconnected(String),
    SetPaused(bool),
    SetWindowFocused(bool),
    SetLearning(bool),
    SelectProfile(String),
    SetForeground(Option<ForegroundApp>),
    TrackedForeground { generation: u64, app: Option<ForegroundApp> },
    RunAction { action: Box<Action>, delay_ms: u32 },
    ReleaseAll,
    LaunchFailed { info: OwnerInfo, description: String, error: String },
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

#[derive(Default)]
struct StatusState {
    current: EngineStatus,
    /// What the sink last received (`None` before the first status).
    emitted: Option<EngineStatus>,
    last_emit: Option<Instant>,
    pending: bool,
    timer: Option<TimerId>,
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
    next_test: u64,
    test_info: OwnerInfo,
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
    fn new(
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
        let cx = Cx::new(start, cfg.settings.clone(), out, Launcher::new(tx.clone()));
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
            next_test: 0,
            test_info: OwnerInfo::test(),
            cx,
        };
        worker.install(&config);
        worker.flush(true);
        worker
    }

    /// One loop iteration. `false` = stop.
    fn iteration(&mut self, received: Result<Envelope, RecvTimeoutError>) -> bool {
        match received {
            Ok(envelope) => {
                let at = envelope.at.max(self.cx.now);
                self.run_timers(at);
                self.cx.now = at;
                if !self.handle(envelope.command) {
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
            Command::LaunchFailed { info, description, error } => {
                self.cx.report(&info, description, Outcome::Failed { error });
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
        }
    }

    /// Active Sensing watchdog: a device that sends it and then goes silent has lost its
    /// connection (MIDI spec: receivers turn their notes off). Release what it was holding.
    fn on_sensing_timer(&mut self, dev: DeviceId, id: TimerId) {
        let state = self.cx.devices.get_mut(dev);
        if state.sensing_timer != Some(id) {
            return;
        }
        state.sensing_timer = None;
        if !state.sensing {
            return;
        }
        let deadline = state.last_heard.map(|at| at + SENSING_TIMEOUT);
        if let Some(deadline) = deadline
            && deadline > self.cx.now
        {
            let timer = self.cx.timers.schedule(deadline, TimerKind::Sensing(dev));
            self.cx.devices.get_mut(dev).sensing_timer = Some(timer);
            return;
        }
        let name = state.name.clone();
        if self.release_device(dev) {
            self.cx.error(format!(
                "\"{name}\" stopped sending (its Active Sensing went silent; was a cable \
                 unplugged?). Everything it was holding has been released."
            ));
        }
    }

    /// Forgets a device's state and releases whatever its notes and controls were holding.
    /// Returns whether it was holding anything.
    fn release_device(&mut self, dev: DeviceId) -> bool {
        let mut released = false;
        for i in 0..self.maps.len() {
            released |=
                behavior::drop_device(&mut self.maps[i], &self.cfg.mappings[i], &mut self.cx, dev);
        }
        let state = self.cx.devices.get_mut(dev);
        state.reset();
        let mut timer = state.sensing_timer.take();
        self.cx.timers.cancel_slot(&mut timer);
        self.cx.dirty = true;
        released
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
                    Request::SwitchProfile(target, info) => self.switch_profile(&target, &info),
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
                        previous.profile_id == m.profile_id && previous.def == m.def
                    })
                })
                .collect()
        };
        let mut kept = vec![false; self.maps.len()];
        for j in carry.iter().flatten() {
            kept[*j] = true;
        }
        for (j, kept) in kept.iter().enumerate() {
            if !kept {
                self.deactivate(j);
                let key = self.maps[j].key;
                self.cx.remove_queue(key);
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
                if !live {
                    self.deactivate(i);
                }
                self.maps[i].live = live;
            }
        }
        if changed {
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

    fn switch_profile(&mut self, target: &ProfileTarget, info: &OwnerInfo) {
        let index = match target {
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

    fn device_disconnected(&mut self, name: &str) {
        if let Some(device) = self.cx.devices.id(name) {
            self.release_device(device);
        }
    }

    // -----------------------------------------------------------------------------------------
    // Actions run from the UI
    // -----------------------------------------------------------------------------------------

    fn run_action(&mut self, action: &Action, delay_ms: u32) {
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
    // MIDI
    // -----------------------------------------------------------------------------------------

    fn on_midi(&mut self, device: String, message: MidiMessage) {
        let dev = self.cx.devices.id_or_insert(&device);
        let now = self.cx.now;
        let state = self.cx.devices.get_mut(dev);
        state.last_heard = Some(now);
        if message == MidiMessage::ActiveSensing {
            state.sensing = true;
        }
        if state.sensing && state.sensing_timer.is_none() {
            let timer = self.cx.timers.schedule(now + SENSING_TIMEOUT, TimerKind::Sensing(dev));
            self.cx.devices.get_mut(dev).sensing_timer = Some(timer);
        }
        let derived = self.cx.devices.get_mut(dev).midi.update(&message);
        let mark = self.cx.outbox.len();
        let mut matched = Vec::new();
        let index = Arc::clone(&self.index);
        self.dispatch(&index, dev, &device, &message, &mut matched);
        for event in derived.iter() {
            self.dispatch_derived(&index, dev, &device, event, &mut matched);
        }
        let log = MidiLog {
            time_us: self.cx.time_us(),
            device,
            message,
            derived: derived.iter().collect(),
            mappings: matched,
        };
        self.cx.outbox.insert(mark, EngineEvent::Midi(log));
    }

    /// Hands one observation to mapping `i` (if live) and runs its behavior.
    fn feed(&mut self, i: usize, inst: InstKey, obs: Obs, ctx: Ctx, matched: &mut Vec<String>) {
        let def = &self.cfg.mappings[i];
        let rt = &mut self.maps[i];
        if !rt.live {
            return;
        }
        let was_active = !rt.instances.is_empty();
        let Some(edge) = rt.observe(inst, obs, def.source.retriggers(), ctx) else {
            return;
        };
        if !matched.iter().any(|id| id.as_str() == &*def.id) {
            matched.push(def.id.to_string());
        }
        behavior::on_edge(rt, def, &mut self.cx, inst, edge, ctx, was_active);
    }

    /// Continuous sources: active while the value is inside the source's range.
    #[allow(clippy::too_many_arguments)]
    fn continuous(
        &mut self,
        list: &[usize],
        name: &str,
        channel: Option<u8>,
        inst: InstKey,
        value: u16,
        ctx: Ctx,
        matched: &mut Vec<String>,
        range_of: impl Fn(&CSourceKind) -> Option<ValueRange>,
    ) {
        for &i in list {
            let source = &self.cfg.mappings[i].source;
            if !source.accepts(name, channel) {
                continue;
            }
            let Some(range) = range_of(&source.kind) else {
                continue;
            };
            let obs = if range.contains(value) { Obs::Active } else { Obs::Inactive };
            self.feed(i, inst, obs, ctx, matched);
        }
    }

    /// Pulse sources (press and release at once) whose filter accepts the message.
    #[allow(clippy::too_many_arguments)]
    fn pulse(
        &mut self,
        list: &[usize],
        name: &str,
        channel: Option<u8>,
        inst: InstKey,
        ctx: Ctx,
        matched: &mut Vec<String>,
        accept: impl Fn(&CSourceKind) -> bool,
    ) {
        for &i in list {
            let source = &self.cfg.mappings[i].source;
            if !source.accepts(name, channel) || !accept(&source.kind) {
                continue;
            }
            self.feed(i, inst, Obs::Pulse, ctx, matched);
        }
    }

    fn dispatch(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        name: &str,
        message: &MidiMessage,
        matched: &mut Vec<String>,
    ) {
        let base = Ctx { device: dev, ..Ctx::default() };
        let system = InstKey::new(dev, 0, 0);
        match *message {
            MidiMessage::NoteOn { channel, note, .. }
            | MidiMessage::NoteOff { channel, note, .. } => {
                let Some(event) = message.note_event() else {
                    return;
                };
                self.cx.devices.get_mut(dev).set_note(channel, note, event.on);
                let ctx = Ctx {
                    channel,
                    note: Some(note),
                    velocity: Some(event.velocity),
                    value: i32::from(event.velocity),
                    ..base
                };
                let inst = InstKey::new(dev, channel, u16::from(note));
                for &i in &index.note {
                    let source = &self.cfg.mappings[i].source;
                    let CSourceKind::Note { notes, velocity } = &source.kind else {
                        continue;
                    };
                    if !source.accepts(name, Some(channel)) || !notes.contains(u16::from(note)) {
                        continue;
                    }
                    let obs = if !event.on {
                        Obs::Inactive
                    } else if velocity.contains(u16::from(event.velocity)) {
                        Obs::Active
                    } else {
                        continue;
                    };
                    self.feed(i, inst, obs, ctx, matched);
                }
                if !event.on {
                    self.after_note_off(index, dev, channel, note, matched);
                }
            }
            MidiMessage::ControlChange { channel, controller, value } => {
                let ctx = Ctx {
                    channel,
                    controller: Some(u16::from(controller)),
                    value: i32::from(value),
                    ..base
                };
                let inst = InstKey::new(dev, channel, u16::from(controller));
                for &i in &index.cc[usize::from(controller & 127)] {
                    let source = &self.cfg.mappings[i].source;
                    let CSourceKind::Cc { range, encoding, .. } = &source.kind else {
                        continue;
                    };
                    if !source.accepts(name, Some(channel)) {
                        continue;
                    }
                    let (obs, ctx) = match encoding {
                        ValueEncoding::Absolute => {
                            let active = range.contains(u16::from(value));
                            (if active { Obs::Active } else { Obs::Inactive }, ctx)
                        }
                        relative => {
                            let ticks = relative_ticks(value, *relative);
                            if ticks == 0 {
                                continue;
                            }
                            (Obs::Ticks(ticks), Ctx { value: ticks, ..ctx })
                        }
                    };
                    self.feed(i, inst, obs, ctx, matched);
                }
                match controller {
                    // All sound off, all notes off, omni/mono/poly: every note is released.
                    120 | 123..=127 => self.release_notes(index, dev, Some(channel), matched),
                    121 => self.reset_controllers(index, dev, channel, matched),
                    _ => {}
                }
            }
            MidiMessage::ProgramChange { channel, program } => {
                let ctx = Ctx { channel, value: i32::from(program), ..base };
                let inst = InstKey::new(dev, channel, 0);
                self.pulse(&index.program, name, Some(channel), inst, ctx, matched, |kind| {
                    matches!(kind, CSourceKind::Program { programs } if programs.contains(u16::from(program)))
                });
            }
            MidiMessage::PitchBend { channel, value } => {
                let ctx = Ctx { channel, value: i32::from(value), ..base };
                let inst = InstKey::new(dev, channel, 0);
                self.continuous(
                    &index.pitch_bend,
                    name,
                    Some(channel),
                    inst,
                    value,
                    ctx,
                    matched,
                    |kind| match kind {
                        CSourceKind::PitchBend { range } => Some(*range),
                        _ => None,
                    },
                );
            }
            MidiMessage::ChannelPressure { channel, pressure } => {
                let ctx = Ctx { channel, value: i32::from(pressure), ..base };
                let inst = InstKey::new(dev, channel, 0);
                self.continuous(
                    &index.channel_pressure,
                    name,
                    Some(channel),
                    inst,
                    u16::from(pressure),
                    ctx,
                    matched,
                    |kind| match kind {
                        CSourceKind::ChannelPressure { range } => Some(*range),
                        _ => None,
                    },
                );
            }
            MidiMessage::PolyPressure { channel, note, pressure } => {
                let ctx = Ctx { channel, note: Some(note), value: i32::from(pressure), ..base };
                let inst = InstKey::new(dev, channel, u16::from(note));
                self.continuous(
                    &index.poly_pressure,
                    name,
                    Some(channel),
                    inst,
                    u16::from(pressure),
                    ctx,
                    matched,
                    |kind| match kind {
                        CSourceKind::PolyPressure { notes, range }
                            if notes.contains(u16::from(note)) =>
                        {
                            Some(*range)
                        }
                        _ => None,
                    },
                );
            }
            MidiMessage::SysEx { ref data } => {
                self.pulse(&index.sysex, name, None, system, base, matched, |kind| {
                    matches!(kind, CSourceKind::SysEx { pattern: Some(pattern) } if pattern.matches(data))
                });
            }
            MidiMessage::Clock => {
                let state = self.cx.devices.get_mut(dev);
                let tick = state.clock_ticks;
                state.clock_ticks += 1;
                self.pulse(&index.clock, name, None, system, base, matched, |kind| {
                    matches!(kind, CSourceKind::Clock { divisor } if tick % u64::from((*divisor).max(1)) == 0)
                });
            }
            MidiMessage::SongSelect { song } => {
                let ctx = Ctx { value: i32::from(song), ..base };
                self.pulse(&index.song_select, name, None, system, ctx, matched, |kind| {
                    matches!(kind, CSourceKind::SongSelect { songs } if songs.contains(u16::from(song)))
                });
            }
            MidiMessage::SongPosition { position } => {
                let ctx = Ctx { value: i32::from(position), ..base };
                self.continuous(
                    &index.song_position,
                    name,
                    None,
                    system,
                    position,
                    ctx,
                    matched,
                    |kind| match kind {
                        CSourceKind::SongPosition { range } => Some(*range),
                        _ => None,
                    },
                );
            }
            MidiMessage::Start => {
                self.cx.devices.get_mut(dev).clock_ticks = 0;
                self.system(index, SystemMessage::Start, name, system, base, matched);
            }
            MidiMessage::Continue => {
                self.system(index, SystemMessage::Continue, name, system, base, matched)
            }
            MidiMessage::Stop => {
                self.system(index, SystemMessage::Stop, name, system, base, matched)
            }
            MidiMessage::TuneRequest => {
                self.system(index, SystemMessage::TuneRequest, name, system, base, matched)
            }
            MidiMessage::ActiveSensing => {
                self.system(index, SystemMessage::ActiveSensing, name, system, base, matched)
            }
            MidiMessage::Reset => {
                self.system(index, SystemMessage::Reset, name, system, base, matched);
                // A reset device has no notes down and its controllers are at their defaults.
                self.release_notes(index, dev, None, matched);
                for channel in 1..=16 {
                    self.reset_controllers(index, dev, channel, matched);
                }
                self.cx.devices.get_mut(dev).clock_ticks = 0;
            }
            MidiMessage::MtcQuarterFrame { .. } | MidiMessage::Unknown { .. } => {}
        }
    }

    fn system(
        &mut self,
        index: &SourceIndex,
        message: SystemMessage,
        name: &str,
        inst: InstKey,
        ctx: Ctx,
        matched: &mut Vec<String>,
    ) {
        self.pulse(&index.system[system_index(message)], name, None, inst, ctx, matched, |_| true);
    }

    fn dispatch_derived(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        name: &str,
        event: Derived,
        matched: &mut Vec<String>,
    ) {
        match event {
            Derived::Parameter(param) => {
                let list = match param.kind {
                    ParameterKind::Nrpn => &index.nrpn,
                    ParameterKind::Rpn => &index.rpn,
                };
                let base = Ctx {
                    device: dev,
                    channel: param.channel,
                    controller: Some(param.number),
                    ..Ctx::default()
                };
                let inst = InstKey::new(dev, param.channel, param.number);
                for &i in list {
                    let source = &self.cfg.mappings[i].source;
                    let CSourceKind::Param { number, range, hires, .. } = &source.kind else {
                        continue;
                    };
                    if *number != param.number || !source.accepts(name, Some(param.channel)) {
                        continue;
                    }
                    let absolute = |value: u16| {
                        let obs = if range.contains(value) { Obs::Active } else { Obs::Inactive };
                        (obs, Ctx { value: i32::from(value), ..base })
                    };
                    let (obs, ctx) = match param.change {
                        ParameterChange::DataMsb if !*hires => absolute(u16::from(param.value7())),
                        ParameterChange::DataLsb if *hires => absolute(param.value14()),
                        ParameterChange::Increment => (Obs::Ticks(1), Ctx { value: 1, ..base }),
                        ParameterChange::Decrement => (Obs::Ticks(-1), Ctx { value: -1, ..base }),
                        _ => continue,
                    };
                    self.feed(i, inst, obs, ctx, matched);
                }
            }
            Derived::ControlChange14(cc) => {
                let ctx = Ctx {
                    device: dev,
                    channel: cc.channel,
                    controller: Some(u16::from(cc.controller)),
                    value: i32::from(cc.value),
                    ..Ctx::default()
                };
                let inst = InstKey::new(dev, cc.channel, u16::from(cc.controller));
                let list = &index.cc14[usize::from(cc.controller & 31)];
                self.continuous(
                    list,
                    name,
                    Some(cc.channel),
                    inst,
                    cc.value,
                    ctx,
                    matched,
                    |kind| match kind {
                        CSourceKind::Cc14 { range, .. } => Some(*range),
                        _ => None,
                    },
                );
            }
        }
    }

    /// After a note off: the released note's aftertouch is gone, and channel pressure cannot
    /// outlast the last held note of its channel (some keyboards never send pressure 0).
    fn after_note_off(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        channel: u8,
        note: u8,
        matched: &mut Vec<String>,
    ) {
        let ctx = Ctx { device: dev, channel, note: Some(note), ..Ctx::default() };
        let inst = InstKey::new(dev, channel, u16::from(note));
        for &i in &index.poly_pressure {
            self.feed(i, inst, Obs::Inactive, ctx, matched);
        }
        if !self.cx.devices.get(dev).any_note_held(channel) {
            let inst = InstKey::new(dev, channel, 0);
            let ctx = Ctx { note: None, ..ctx };
            for &i in &index.channel_pressure {
                self.feed(i, inst, Obs::Inactive, ctx, matched);
            }
        }
    }

    /// Releases every active note (and aftertouch) instance of the device, on one channel or
    /// all, exactly as if the note offs had arrived.
    fn release_notes(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        channel: Option<u8>,
        matched: &mut Vec<String>,
    ) {
        self.cx.devices.get_mut(dev).clear_notes(channel);
        let lists = [&index.note, &index.poly_pressure, &index.channel_pressure];
        for &i in lists.into_iter().flatten() {
            let active = self.maps[i]
                .active(|key| key.device() == dev && channel.is_none_or(|c| key.channel() == c));
            for (key, ctx) in active {
                let ctx = Ctx { velocity: ctx.velocity.map(|_| 0), value: 0, ..ctx };
                self.feed(i, key, Obs::Inactive, ctx, matched);
            }
        }
    }

    /// Reset All Controllers (CC 121): controls that reset to a value outside a mapping's range
    /// release, as if that value had arrived (the RP-015 set, as tracked by `MidiState`).
    fn reset_controllers(
        &mut self,
        index: &SourceIndex,
        dev: DeviceId,
        channel: u8,
        matched: &mut Vec<String>,
    ) {
        self.release_outside(&index.pitch_bend, dev, channel, None, 8192, matched);
        self.release_outside(&index.channel_pressure, dev, channel, None, 0, matched);
        self.release_outside(&index.poly_pressure, dev, channel, None, 0, matched);
        for (controller, value) in [(1u16, 0u16), (11, 127), (64, 0), (65, 0), (66, 0), (67, 0)] {
            let list = &index.cc[usize::from(controller)];
            self.release_outside(list, dev, channel, Some(controller), value, matched);
        }
        for (controller, value) in [(1u16, 0u16), (11, 127 << 7)] {
            let list = &index.cc14[usize::from(controller)];
            self.release_outside(list, dev, channel, Some(controller), value, matched);
        }
    }

    fn release_outside(
        &mut self,
        list: &[usize],
        dev: DeviceId,
        channel: u8,
        number: Option<u16>,
        value: u16,
        matched: &mut Vec<String>,
    ) {
        for &i in list {
            let range = match &self.cfg.mappings[i].source.kind {
                CSourceKind::PitchBend { range }
                | CSourceKind::ChannelPressure { range }
                | CSourceKind::PolyPressure { range, .. }
                | CSourceKind::Cc { range, encoding: ValueEncoding::Absolute, .. }
                | CSourceKind::Cc14 { range, .. } => *range,
                _ => continue,
            };
            if range.contains(value) {
                continue;
            }
            let active = self.maps[i].active(|key| key.device() == dev && key.channel() == channel);
            for (key, ctx) in active {
                if number.is_some_and(|n| ctx.controller != Some(n)) {
                    continue;
                }
                self.feed(i, key, Obs::Inactive, Ctx { value: i32::from(value), ..ctx }, matched);
            }
        }
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
        if !self.cx.outbox.is_empty() {
            let mut events = std::mem::take(&mut self.cx.outbox);
            for event in events.drain(..) {
                self.emit(event);
            }
            if self.cx.outbox.is_empty() {
                self.cx.outbox = events;
            }
        }
        if self.cx.out.changed {
            self.cx.out.changed = false;
            self.cx.dirty = true;
        }
        if self.cx.dirty {
            self.cx.dirty = false;
            let status = self.build_status();
            if status != self.status.current {
                *self.shared.status.lock().unwrap_or_else(|e| e.into_inner()) = status.clone();
                self.status.current = status;
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
                if self.status.emitted.as_ref() != Some(&self.status.current) {
                    self.status.last_emit = Some(now);
                    self.status.emitted = Some(self.status.current.clone());
                    self.emit(EngineEvent::Status(self.status.current.clone()));
                }
            } else if self.status.timer.is_none() {
                self.status.timer = Some(self.cx.timers.schedule(due, TimerKind::Status));
            }
        }
    }

    fn build_status(&self) -> EngineStatus {
        let id = |index: Option<usize>| index.map(|i| self.cfg.profiles[i].id.to_string());
        EngineStatus {
            paused: self.cx.gate.paused,
            focus_suspended: self.cx.gate.focus_suspended(),
            learning: self.cx.gate.learning,
            selected_profile: id(self.profiles.selected),
            effective_profile: id(self.profiles.effective),
            auto_switched: self.profiles.auto_switched,
            layers: self.cx.layers.names(),
            held_keys: self.cx.out.held_keys(),
            engaged_mappings: self
                .maps
                .iter()
                .zip(&self.cfg.mappings)
                .filter(|(rt, _)| rt.engaged())
                .map(|(_, def)| def.id.to_string())
                .collect(),
            foreground: self.cx.foreground.as_ref().map(|f| f.app.clone()),
            backend: self.cx.out.name().to_string(),
        }
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
        self.tests.clear();
        self.status.timer = None;
        self.status.pending = true;
        self.cx.dirty = true;
    }

    /// Final cleanup on the engine thread: release everything and deliver the last status.
    fn stop(&mut self) {
        let _ = catch_unwind(AssertUnwindSafe(|| {
            self.release_outputs(true);
            self.tracker = None;
            self.cx.timers.clear();
            self.flush(true);
        }));
    }
}

/// The selected profile: `wanted` if it exists, else the first non-always-on profile, else the
/// first profile.
fn resolve_selected(cfg: &CompiledConfig, wanted: Option<&str>) -> Option<usize> {
    wanted
        .and_then(|id| cfg.profile_index(id))
        .or_else(|| cfg.profiles.iter().position(|p| !p.always_on))
        .or(if cfg.profiles.is_empty() { None } else { Some(0) })
}
