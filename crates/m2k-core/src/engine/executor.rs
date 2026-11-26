//! Playing output over time, without ever sleeping:
//!
//! - **Action queues.** Each owner (a mapping, or the UI's Test button) has a bounded FIFO of
//!   actions. The head runs step by step on timers (key downs, `tap_ms`, key ups, macro delays,
//!   text typed character by character); when it finishes, the next one starts after a small
//!   gap, so a burst of encoder ticks yields exactly that many distinct key presses. Different
//!   owners run concurrently. An aborted action releases what it holds; so does an action that
//!   completes with keys still down (a macro's `KeyDown` without `KeyUp`).
//! - **Hold engagements.** Keys of a held target go down in order and come up in reverse order,
//!   `key_stagger_ms` apart; a release during the press sequence reverses from wherever it got.
//! - **Typematic.** One global slot emulating keyboard auto-repeat for backends that do not
//!   repeat injected keys: the most recently pressed non-modifier key of a `Hold` is pressed
//!   again after `repeat_delay_ms`, then every `repeat_interval_ms`, until it is released or any
//!   other key is pressed.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use m2k_output::{Key, MouseButton};

use super::compile::{ActionKind, CAction, MIN_GAP_MS, Prim};
use super::cx::{Ctx, Cx, OwnerInfo, TimerKind};
use super::text::shorten;
use super::timers::TimerId;

/// Actions waiting behind the running one, per owner. Further firings are dropped (newest
/// first) and reported as rate limited.
pub const QUEUE_LIMIT: usize = 64;

#[derive(Default)]
pub struct Executor {
    queues: HashMap<u64, Queue>,
    holds: HashMap<u64, HoldProc>,
}

struct Queue {
    info: OwnerInfo,
    running: Option<Running>,
    pending: VecDeque<(CAction, Ctx)>,
    /// The next action may not start before this (the gap after the previous one).
    ready_at: Option<Instant>,
    timer: Option<TimerId>,
    overflow_reported: Option<Instant>,
}

struct Running {
    action: CAction,
    ctx: Ctx,
    /// Next primitive step.
    pc: usize,
    /// Text being typed in pieces (one character at a time with `char_delay_ms`, or in chunks
    /// for backends that cannot take long text at once).
    typing: Option<Typing>,
    /// Keys and buttons this action holds, in press order.
    keys: Vec<Key>,
    buttons: Vec<MouseButton>,
}

struct HoldProc {
    info: OwnerInfo,
    keys: Arc<[Key]>,
    /// `keys[..pressed]` are down.
    pressed: usize,
    /// Goal: pressing towards all keys down, or releasing towards none.
    engaged: bool,
    auto_repeat: bool,
    timer: Option<TimerId>,
}

struct Typing {
    text: String,
    /// Byte offset of the next piece.
    offset: usize,
    /// Characters per piece.
    chars: usize,
    /// Pause between pieces.
    delay_ms: u32,
}

/// The Linux uinput backend loses events when one call queues too many (the kernel buffers
/// about 128 events per reader): long text goes out in chunks of this many characters...
const PACED_TEXT_CHARS: usize = 25;
/// ...this far apart.
const PACED_TEXT_GAP_MS: u32 = 10;

/// Byte offset `chars` characters after `offset` (or the end of `text`).
fn advance_chars(text: &str, offset: usize, chars: usize) -> usize {
    text[offset..].char_indices().nth(chars).map_or(text.len(), |(i, _)| offset + i)
}

pub struct Typematic {
    owner: u64,
    key: Key,
    timer: Option<TimerId>,
}

pub enum Enqueued {
    Queued,
    Full,
}

enum Progress {
    Wait(u32),
    Done,
}

impl Executor {
    pub fn is_full(&self, owner: u64) -> bool {
        self.queues.get(&owner).is_some_and(|q| q.pending.len() >= QUEUE_LIMIT)
    }

    /// Something is queued behind the running action.
    pub fn has_pending(&self, owner: u64) -> bool {
        self.queues.get(&owner).is_some_and(|q| !q.pending.is_empty())
    }

    /// Rate limits queue overflow reports; `true` when this one should be reported.
    pub fn note_overflow(&mut self, owner: u64, now: Instant) -> bool {
        let Some(queue) = self.queues.get_mut(&owner) else {
            return true;
        };
        if queue
            .overflow_reported
            .is_some_and(|at| now.saturating_duration_since(at) < Duration::from_millis(500))
        {
            return false;
        }
        queue.overflow_reported = Some(now);
        true
    }
}

impl Running {
    fn new(action: CAction, ctx: Ctx) -> Self {
        Running { action, ctx, pc: 0, typing: None, keys: Vec::new(), buttons: Vec::new() }
    }
}

impl Cx {
    fn gap(&self) -> Duration {
        Cx::ms(self.settings.key_stagger_ms.max(MIN_GAP_MS))
    }

    // ----- single key operations (all key presses of the engine go through here) -----

    pub fn press_key(&mut self, info: &OwnerInfo, key: Key) {
        // A real keyboard stops repeating the previous key when another key goes down.
        self.typematic_cancel();
        if let Err(error) = self.out.press_key(key, info.owner) {
            self.report_failure(info, format!("press {}", key.label()), error);
        }
    }

    pub fn release_key(&mut self, info: &OwnerInfo, key: Key) {
        if let Err(error) = self.out.release_key(key, info.owner) {
            self.report_failure(info, format!("release {}", key.label()), error);
        }
        if self.out.key_count(key) == 0 && self.typematic.as_ref().is_some_and(|t| t.key == key) {
            self.typematic_cancel();
        }
    }

    fn type_text(&mut self, info: &OwnerInfo, text: &str) {
        self.typematic_cancel();
        if let Err(error) = self.out.text(text) {
            self.report_failure(info, format!("type {:?}", shorten(text, 40)), error);
        }
    }

    // ----- action queues -----

    /// Queues an output action for `info.owner` and starts it if the queue is idle.
    pub fn enqueue(&mut self, info: &OwnerInfo, action: &CAction, ctx: Ctx) -> Enqueued {
        let owner = info.owner;
        let queue = self.exec.queues.entry(owner).or_insert_with(|| Queue {
            info: info.clone(),
            running: None,
            pending: VecDeque::new(),
            ready_at: None,
            timer: None,
            overflow_reported: None,
        });
        if queue.pending.len() >= QUEUE_LIMIT {
            return Enqueued::Full;
        }
        queue.pending.push_back((action.clone(), ctx));
        if queue.running.is_none() && queue.timer.is_none() {
            self.advance_queue(owner);
        }
        Enqueued::Queued
    }

    pub fn on_queue_timer(&mut self, owner: u64, id: TimerId) {
        let Some(queue) = self.exec.queues.get_mut(&owner) else {
            return;
        };
        if queue.timer != Some(id) {
            return;
        }
        queue.timer = None;
        self.advance_queue(owner);
    }

    fn advance_queue(&mut self, owner: u64) {
        loop {
            let (running, info) = match self.exec.queues.get_mut(&owner) {
                Some(queue) => (queue.running.take(), queue.info.clone()),
                None => return,
            };
            if let Some(mut running) = running {
                match self.step(&info, &mut running) {
                    Progress::Wait(ms) => {
                        let at = self.actual_now() + Cx::ms(ms);
                        let timer = self.timers.schedule(at, TimerKind::Queue(owner));
                        match self.exec.queues.get_mut(&owner) {
                            Some(queue) => {
                                queue.running = Some(running);
                                queue.timer = Some(timer);
                            }
                            None => {
                                self.timers.cancel(timer);
                                self.finish(&info, running);
                            }
                        }
                        return;
                    }
                    Progress::Done => {
                        self.finish(&info, running);
                        let ready = self.actual_now() + self.gap();
                        if let Some(queue) = self.exec.queues.get_mut(&owner) {
                            queue.ready_at = Some(ready);
                        }
                    }
                }
            }

            let now = self.actual_now();
            let Some(queue) = self.exec.queues.get_mut(&owner) else {
                return;
            };
            if queue.pending.is_empty() {
                return;
            }
            if let Some(ready) = queue.ready_at
                && ready > now
            {
                queue.timer = Some(self.timers.schedule(ready, TimerKind::Queue(owner)));
                return;
            }
            let Some((action, ctx)) = queue.pending.pop_front() else {
                return;
            };
            queue.running = Some(Running::new(action, ctx));
        }
    }

    /// Executes primitive steps until the action has to wait or is done.
    fn step(&mut self, info: &OwnerInfo, run: &mut Running) -> Progress {
        if let Some(mut typing) = run.typing.take() {
            let end = advance_chars(&typing.text, typing.offset, typing.chars);
            self.type_text(info, &typing.text[typing.offset..end]);
            if end < typing.text.len() {
                typing.offset = end;
                let delay = typing.delay_ms;
                run.typing = Some(typing);
                return Progress::Wait(delay);
            }
        }

        let action = run.action.clone();
        let ActionKind::Output(steps) = &action.kind else {
            return Progress::Done;
        };
        while let Some(prim) = steps.get(run.pc) {
            run.pc += 1;
            match prim {
                Prim::Down(key) => {
                    self.press_key(info, *key);
                    run.keys.push(*key);
                }
                Prim::Up(key) => {
                    if let Some(pos) = run.keys.iter().rposition(|k| k == key) {
                        run.keys.remove(pos);
                        self.release_key(info, *key);
                    } else if let Err(error) = self.out.key_up_unheld(*key) {
                        self.report_failure(info, format!("release {}", key.label()), error);
                    }
                }
                Prim::Text { text, char_delay_ms } => {
                    let rendered = text.render(&run.ctx, self.settings.middle_c_octave);
                    let (chars, delay_ms) = if *char_delay_ms > 0 {
                        (1, *char_delay_ms)
                    } else if self.out.paces_text() {
                        (PACED_TEXT_CHARS, PACED_TEXT_GAP_MS)
                    } else {
                        (usize::MAX, 0)
                    };
                    let end = advance_chars(&rendered, 0, chars);
                    self.type_text(info, &rendered[..end]);
                    if end < rendered.len() {
                        let text = rendered.into_owned();
                        run.typing = Some(Typing { text, offset: end, chars, delay_ms });
                        return Progress::Wait(delay_ms);
                    }
                }
                Prim::ButtonDown(button) => {
                    self.press_button(info, *button);
                    run.buttons.push(*button);
                }
                Prim::ButtonUp(button) => {
                    if let Some(pos) = run.buttons.iter().rposition(|b| b == button) {
                        run.buttons.remove(pos);
                        self.release_button(info, *button);
                    } else if let Err(error) = self.out.button_up_unheld(*button) {
                        let what =
                            format!("release {} button", super::compile::button_name(*button));
                        self.report_failure(info, what, error);
                    }
                }
                Prim::Scroll { dx, dy } => {
                    if let Err(error) = self.out.scroll(*dx, *dy) {
                        self.report_failure(info, action.label().to_string(), error);
                    }
                }
                Prim::Move { x, y, absolute } => {
                    if let Err(error) = self.out.move_pointer(*x, *y, *absolute) {
                        self.report_failure(info, action.label().to_string(), error);
                    }
                }
                Prim::Wait(ms) => {
                    if *ms > 0 {
                        return Progress::Wait(*ms);
                    }
                }
            }
        }
        Progress::Done
    }

    /// Releases whatever a finished or aborted action still holds (reverse order).
    fn finish(&mut self, info: &OwnerInfo, mut run: Running) {
        while let Some(key) = run.keys.pop() {
            self.release_key(info, key);
        }
        while let Some(button) = run.buttons.pop() {
            self.release_button(info, button);
        }
    }

    /// Stops the owner's running action (releasing what it holds) and drops what is queued.
    pub fn abort_queue(&mut self, owner: u64) {
        let Some(queue) = self.exec.queues.get_mut(&owner) else {
            return;
        };
        self.timers.cancel_slot(&mut queue.timer);
        queue.pending.clear();
        let running = queue.running.take();
        let info = queue.info.clone();
        if let Some(running) = running {
            self.finish(&info, running);
            let ready = self.actual_now() + self.gap();
            if let Some(queue) = self.exec.queues.get_mut(&owner) {
                queue.ready_at = Some(ready);
            }
        }
    }

    /// Aborts and forgets the owner's queue (the owner is gone).
    pub fn remove_queue(&mut self, owner: u64) {
        self.abort_queue(owner);
        self.exec.queues.remove(&owner);
    }

    pub fn abort_all_queues(&mut self) {
        let owners: Vec<u64> = self.exec.queues.keys().copied().collect();
        for owner in owners {
            self.abort_queue(owner);
        }
    }

    // ----- hold engagements -----

    pub fn hold_keys(&mut self, info: &OwnerInfo, keys: &Arc<[Key]>, auto_repeat: bool) {
        let owner = info.owner;
        let proc = self.exec.holds.entry(owner).or_insert_with(|| HoldProc {
            info: info.clone(),
            keys: keys.clone(),
            pressed: 0,
            engaged: false,
            auto_repeat,
            timer: None,
        });
        proc.engaged = true;
        proc.auto_repeat = auto_repeat;
        self.timers.cancel_slot(&mut proc.timer);
        self.advance_hold(owner);
    }

    /// Releases a hold engagement: in reverse order with the key stagger, or all at once.
    pub fn unhold_keys(&mut self, owner: u64, immediate: bool) {
        let Some(proc) = self.exec.holds.get_mut(&owner) else {
            return;
        };
        proc.engaged = false;
        self.timers.cancel_slot(&mut proc.timer);
        if immediate {
            if let Some(proc) = self.exec.holds.remove(&owner) {
                for key in proc.keys[..proc.pressed].iter().rev() {
                    self.release_key(&proc.info, *key);
                }
            }
            return;
        }
        self.advance_hold(owner);
    }

    /// Ends every hold engagement at once (forced release).
    pub fn release_all_holds(&mut self) {
        let owners: Vec<u64> = self.exec.holds.keys().copied().collect();
        for owner in owners {
            self.unhold_keys(owner, true);
        }
    }

    pub fn on_hold_timer(&mut self, owner: u64, id: TimerId) {
        let Some(proc) = self.exec.holds.get_mut(&owner) else {
            return;
        };
        if proc.timer != Some(id) {
            return;
        }
        proc.timer = None;
        self.advance_hold(owner);
    }

    fn advance_hold(&mut self, owner: u64) {
        let stagger = self.settings.key_stagger_ms;
        loop {
            let Some(proc) = self.exec.holds.get_mut(&owner) else {
                return;
            };
            let more = if proc.engaged && proc.pressed < proc.keys.len() {
                let key = proc.keys[proc.pressed];
                proc.pressed += 1;
                let more = proc.pressed < proc.keys.len();
                let info = proc.info.clone();
                let repeat = proc.auto_repeat && !key.is_modifier();
                self.press_key(&info, key);
                if repeat {
                    self.typematic_start(owner, key);
                }
                more
            } else if !proc.engaged && proc.pressed > 0 {
                proc.pressed -= 1;
                let key = proc.keys[proc.pressed];
                let more = proc.pressed > 0;
                let info = proc.info.clone();
                self.release_key(&info, key);
                more
            } else {
                if !proc.engaged {
                    self.exec.holds.remove(&owner);
                }
                return;
            };
            if more && stagger > 0 {
                let at = self.actual_now() + Cx::ms(stagger);
                let timer = self.timers.schedule(at, TimerKind::Hold(owner));
                if let Some(proc) = self.exec.holds.get_mut(&owner) {
                    proc.timer = Some(timer);
                }
                return;
            }
        }
    }

    // ----- typematic -----

    fn typematic_start(&mut self, owner: u64, key: Key) {
        self.typematic_cancel();
        let at = self.actual_now() + Cx::ms(self.settings.repeat_delay_ms);
        let timer = self.timers.schedule(at, TimerKind::Typematic);
        self.typematic = Some(Typematic { owner, key, timer: Some(timer) });
    }

    pub fn typematic_cancel(&mut self) {
        if let Some(mut typematic) = self.typematic.take() {
            self.timers.cancel_slot(&mut typematic.timer);
        }
    }

    pub fn on_typematic_timer(&mut self, id: TimerId) {
        let Some(typematic) = &self.typematic else {
            return;
        };
        if typematic.timer != Some(id) {
            return;
        }
        let (owner, key) = (typematic.owner, typematic.key);
        let info = match self.exec.holds.get(&owner) {
            Some(proc) if proc.engaged && self.out.key_count(key) > 0 => proc.info.clone(),
            _ => {
                self.typematic = None;
                return;
            }
        };
        if let Err(error) = self.out.repeat_key(key) {
            self.report_failure(&info, format!("repeat {}", key.label()), error);
        }
        let interval = Cx::ms(self.settings.repeat_interval_ms.max(1));
        let mut next = id.at() + interval;
        let real = Instant::now();
        if next < real {
            // Fell behind (the system was busy): continue the cadence, do not burst.
            next = real + interval;
        }
        let timer = self.timers.schedule(next, TimerKind::Typematic);
        if let Some(typematic) = &mut self.typematic {
            typematic.timer = Some(timer);
        }
    }
}
