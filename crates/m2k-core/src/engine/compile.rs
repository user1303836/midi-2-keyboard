//! Compiles a [`Config`] into the form the engine runs: actions become primitive timed steps
//! (tap duration and key stagger already applied), SysEx and app patterns are parsed once,
//! labels for the monitor are prepared, and [`SourceIndex`] maps each message kind to the live
//! mappings that can match it, so the hot path never scans the whole config.

use std::sync::Arc;

use m2k_midi::ParameterKind;
use m2k_output::{Key, MouseButton};

use super::cx::Ctx;
use super::midi_out::{CSend, parse_raw};
use super::pattern::{AppRule, SysExPattern};
use super::source::Obs;
use super::text::{TextTemplate, shorten, type_label};
use crate::model::*;

/// Minimum pause between the key-up of one queued action and the key-down of the next, so that
/// every tap of a burst registers as a distinct press.
pub const MIN_GAP_MS: u32 = 2;

/// Pause between the clicks of a double/triple click.
const CLICK_GAP_MS: u32 = 15;

/// How an action or hold target is gated (see `Gate::check`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Keys, text, mouse, launching programs. Suppressed while learning, paused or while the
    /// app's own window has focus.
    Os,
    /// Profile switches and layers. Suppressed while learning or paused.
    Internal,
    /// Pause / resume / release all. Suppressed only while learning.
    Engine,
}

pub struct CompiledConfig {
    pub settings: Settings,
    pub profiles: Vec<CProfile>,
    /// Every enabled mapping of every profile, in config order.
    pub mappings: Vec<CMapping>,
    /// Some profile rule or condition depends on the foreground application.
    pub needs_foreground: bool,
    /// Problems the user should hear about once (invalid SysEx patterns, ...).
    pub errors: Vec<String>,
    /// `config.selected_profile` exactly as given.
    pub selected_profile: Option<String>,
}

impl CompiledConfig {
    pub fn profile_index(&self, id: &str) -> Option<usize> {
        self.profiles.iter().position(|p| &*p.id == id)
    }
}

pub struct CProfile {
    pub id: Arc<str>,
    pub name: String,
    pub always_on: bool,
    pub apps: Vec<AppRule>,
}

pub struct CMapping {
    pub id: Arc<str>,
    /// Index into [`CompiledConfig::profiles`].
    pub profile: usize,
    pub profile_id: Arc<str>,
    /// The definition as configured. A mapping whose definition is unchanged keeps its runtime
    /// state across a config reload.
    pub def: Mapping,
    pub source: CSource,
    pub behavior: CBehavior,
    pub conditions: Vec<CCondition>,
    pub rate_limit: Option<RateLimit>,
    /// `None` also when both halves are empty.
    pub feedback: Option<CFeedback>,
}

/// A compiled [`Feedback`]: an empty (or invalid) half is `None` and sends nothing.
pub struct CFeedback {
    pub device: Option<Arc<str>>,
    pub when: CWhen,
    pub on: Option<CSend>,
    pub off: Option<CSend>,
}

/// [`FeedbackWhen`].
pub enum CWhen {
    Engaged,
    Layer(Arc<str>),
    /// A profile id.
    Profile(Arc<str>),
}

pub struct CSource {
    pub device: Option<Box<str>>,
    pub channel: Option<u8>,
    pub kind: CSourceKind,
    /// [`Source::hysteresis`] (0 for sources it does not apply to).
    pub hysteresis: u16,
    /// A repeat of the same in-range value is a fresh press (press-only buttons).
    pub presses_on_repeat: bool,
    /// [`Source::debounce_ms`] (0 for sources it does not apply to).
    pub debounce_ms: u32,
    /// A ranged, absolute value (what a `Change` jitter guard applies to).
    pub ranged: bool,
}

impl CSource {
    /// Device and channel filter. `channel` is `None` for system messages (always accepted).
    pub fn accepts(&self, device: &str, channel: Option<u8>) -> bool {
        self.device.as_deref().is_none_or(|d| d == device)
            && match (self.channel, channel) {
                (Some(want), Some(got)) => want == got,
                _ => true,
            }
    }

    /// Whether a second "active" message for an already active instance is a fresh press (a
    /// note retriggered without note off) rather than a value update.
    pub fn retriggers(&self) -> bool {
        matches!(self.kind, CSourceKind::Note { .. } | CSourceKind::Chord { .. })
    }

    /// The observation for an absolute `value` of a ranged source: it activates when the value
    /// enters `range` and, with hysteresis, deactivates only once it leaves the widened band.
    pub fn level(&self, range: ValueRange, value: u16) -> Obs {
        let inside = range.contains(value);
        let band = inside
            || (self.hysteresis > 0
                && value >= range.min.saturating_sub(self.hysteresis)
                && value <= range.max.saturating_add(self.hysteresis));
        Obs::Level { inside, band }
    }
}

pub enum CSourceKind {
    Note {
        notes: ValueRange,
        velocity: ValueRange,
    },
    /// `notes` sorted and unique; `mask` has a bit per note (0 = empty chord, never matches).
    Chord {
        notes: Vec<u8>,
        mask: u128,
        window_ms: u32,
    },
    Cc {
        controller: u8,
        range: ValueRange,
        encoding: ValueEncoding,
    },
    Cc14 {
        controller: u8,
        range: ValueRange,
    },
    Param {
        kind: ParameterKind,
        number: u16,
        range: ValueRange,
        hires: bool,
    },
    Program {
        programs: ValueRange,
    },
    PitchBend {
        range: ValueRange,
    },
    ChannelPressure {
        range: ValueRange,
    },
    PolyPressure {
        notes: ValueRange,
        range: ValueRange,
    },
    /// `None` when the configured pattern is invalid: never matches (reported once).
    SysEx {
        pattern: Option<SysExPattern>,
    },
    Clock {
        divisor: u16,
    },
    SongSelect {
        songs: ValueRange,
    },
    SongPosition {
        range: ValueRange,
    },
    System {
        message: SystemMessage,
    },
}

pub enum CBehavior {
    Trigger {
        on_press: Option<CAction>,
        on_release: Option<CAction>,
        repeat: Option<Repeat>,
    },
    /// `pulse` is what a pulse source (press and release in the same instant) does: a tap of the
    /// target, so the press is long enough to register.
    Hold {
        target: CHoldTarget,
        auto_repeat: bool,
        pulse: Option<CAction>,
    },
    Toggle {
        target: CHoldTarget,
    },
    Cycle {
        actions: Vec<CAction>,
        reset_after_ms: Option<u32>,
    },
    TapHold {
        tap: Option<CAction>,
        hold: Option<CAction>,
        hold_target: Option<CHoldTarget>,
        hold_ms: u32,
    },
    /// `highest` = the largest press count with an action (0 = nothing configured).
    MultiTap {
        actions: [Option<CAction>; 3],
        highest: u8,
        window_ms: u32,
        /// The long-press action; with it, taps count on release.
        hold: Option<CAction>,
        hold_ms: u32,
    },
    /// `min_change` > 1: jitter guard for ranged absolute sources.
    Change {
        action: CAction,
        min_change: u16,
    },
    /// The mapping's only action or target is empty: it matches, but never does anything.
    Inert,
    Step {
        up: Option<CAction>,
        down: Option<CAction>,
        step: u16,
        max_burst: u16,
        invert: bool,
        fast: Option<CFastStep>,
    },
}

/// [`FastStep`]: one coarse action instead of `ticks` or more fine ones.
pub struct CFastStep {
    pub ticks: u32,
    pub up: Option<CAction>,
    pub down: Option<CAction>,
}

pub enum CCondition {
    /// `process` / `title`: the patterns as written, for messages.
    App {
        rule: AppRule,
        negate: bool,
        process: String,
        title: String,
    },
    Layer {
        layer: String,
        active: bool,
    },
    NoteHeld {
        channel: Option<u8>,
        note: u8,
        held: bool,
    },
    ControlValue {
        channel: Option<u8>,
        controller: u8,
        range: ValueRange,
    },
}

// ---------------------------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------------------------

pub type CAction = Arc<CompiledAction>;

pub struct CompiledAction {
    pub kind: ActionKind,
    pub class: Class,
    /// Roughly how long the action takes to play (its delays), in milliseconds.
    pub est_ms: u32,
    label: String,
    /// Set when the label depends on the triggering message (text with placeholders).
    dynamic: Option<TextTemplate>,
}

impl CompiledAction {
    /// Short human description for the monitor.
    pub fn describe(&self, ctx: &Ctx, middle_c_octave: i8) -> String {
        if let ActionKind::Midi(midi) = &self.kind {
            return midi.describe(None, ctx.value7, middle_c_octave);
        }
        match &self.dynamic {
            Some(template) => type_label(&template.render(ctx, middle_c_octave)),
            None => self.label.clone(),
        }
    }

    /// The description without trigger data (for errors reported later).
    /// Plays through the owner's action queue (OS output).
    pub fn queues(&self) -> bool {
        matches!(self.kind, ActionKind::Output(_))
    }

    pub fn label(&self) -> &str {
        &self.label
    }
}

pub enum ActionKind {
    /// OS input, played step by step through the owner's action queue.
    Output(Vec<Prim>),
    SwitchProfile(ProfileTarget),
    /// Cannot be performed with this config (switch to a profile that does not exist): firing it
    /// reports this error.
    Unavailable(String),
    Layer {
        layer: Arc<str>,
        op: LayerOp,
    },
    Engine(EngineOp),
    Launch(Launch),
    /// MIDI to an output port, sent at once (not through the action queue, so forwarding a
    /// knob is never slowed down by key gaps).
    Midi(CMidi),
}

/// A compiled "Send MIDI": the output (`None` = the one named like the device the triggering
/// message came from) and the message.
#[derive(Debug, Clone)]
pub struct CMidi {
    pub device: Option<Arc<str>>,
    pub send: CSend,
}

impl CMidi {
    /// `send CC 20 = 127 (ch 1) → "Faderfox EC4"`; `port` = where it actually goes, when known.
    pub fn describe(&self, port: Option<&str>, incoming: u8, middle_c_octave: i8) -> String {
        let text = self.send.text(incoming, middle_c_octave);
        match port.or(self.device.as_deref()) {
            Some(port) => format!("send {text} \u{2192} \"{port}\""),
            None => format!("send {text}"),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Launch {
    Run { command: String, args: Vec<String>, working_dir: String },
    Open { target: String },
}

/// One primitive step of an output action.
pub enum Prim {
    /// Press a key (reference counted: always sends a key down).
    Down(Key),
    /// Release a key this action pressed.
    Up(Key),
    Text {
        text: TextTemplate,
        char_delay_ms: u32,
    },
    ButtonDown(MouseButton),
    ButtonUp(MouseButton),
    Scroll {
        dx: i32,
        dy: i32,
    },
    Move {
        x: i32,
        y: i32,
        absolute: bool,
    },
    Wait(u32),
    /// A "Send MIDI" macro step.
    Midi(CMidi),
}

/// A compiled [`HoldTarget`] with its monitor labels.
pub struct CHoldTarget {
    pub what: HoldWhat,
    pub hold_label: String,
    pub release_label: String,
}

pub enum HoldWhat {
    Keys(Arc<[Key]>),
    Button(MouseButton),
    Layer(Arc<str>),
}

impl CHoldTarget {
    pub fn class(&self) -> Class {
        match self.what {
            HoldWhat::Layer(_) => Class::Internal,
            HoldWhat::Keys(_) | HoldWhat::Button(_) => Class::Os,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Compilation
// ---------------------------------------------------------------------------------------------

pub fn compile(config: &Config) -> CompiledConfig {
    let settings = config.settings.clone();
    let profiles: Vec<CProfile> = config
        .profiles
        .iter()
        .map(|p| CProfile {
            id: Arc::from(p.id.as_str()),
            name: p.name.clone(),
            always_on: p.always_on,
            apps: p.apps.iter().map(AppRule::new).collect(),
        })
        .collect();

    let mut errors = Vec::new();
    let mut mappings = Vec::new();
    for (pi, profile) in config.profiles.iter().enumerate() {
        for mapping in profile.mappings.iter().filter(|m| m.enabled) {
            mappings.push(compile_mapping(pi, &profiles, mapping, &settings, &mut errors));
        }
    }

    let needs_foreground = (settings.auto_switch_profiles
        && profiles.iter().any(|p| !p.always_on && !p.apps.is_empty()))
        || mappings
            .iter()
            .any(|m| m.conditions.iter().any(|c| matches!(c, CCondition::App { .. })));

    CompiledConfig {
        settings,
        profiles,
        mappings,
        needs_foreground,
        errors,
        selected_profile: config.selected_profile.clone(),
    }
}

fn compile_mapping(
    pi: usize,
    profiles: &[CProfile],
    mapping: &Mapping,
    settings: &Settings,
    errors: &mut Vec<String>,
) -> CMapping {
    let label = if mapping.name.trim().is_empty() { &mapping.id } else { &mapping.name };
    let act = |a: &Action| action(a, settings, profiles);
    // An empty action (no keys, no text, ...) is no action: the slot means "nothing".
    let act_opt = |a: &Option<Action>| a.as_ref().filter(|a| !a.is_empty()).map(act);
    let kind = match &mapping.source.event {
        SourceEvent::Note { notes, velocity } => {
            CSourceKind::Note { notes: *notes, velocity: *velocity }
        }
        SourceEvent::Chord { notes, window_ms } => {
            let mut notes: Vec<u8> = notes.iter().copied().filter(|n| *n < 128).collect();
            notes.sort_unstable();
            notes.dedup();
            let mask = notes.iter().fold(0u128, |mask, n| mask | (1u128 << n));
            CSourceKind::Chord { notes, mask, window_ms: *window_ms }
        }
        SourceEvent::ControlChange { controller, range, encoding } => CSourceKind::Cc {
            controller: (*controller).min(127),
            range: *range,
            encoding: *encoding,
        },
        SourceEvent::ControlChange14 { controller, range } => {
            CSourceKind::Cc14 { controller: (*controller).min(31), range: *range }
        }
        SourceEvent::Nrpn { number, range, hires } => CSourceKind::Param {
            kind: ParameterKind::Nrpn,
            number: *number,
            range: *range,
            hires: *hires,
        },
        SourceEvent::Rpn { number, range, hires } => CSourceKind::Param {
            kind: ParameterKind::Rpn,
            number: *number,
            range: *range,
            hires: *hires,
        },
        SourceEvent::ProgramChange { programs } => CSourceKind::Program { programs: *programs },
        SourceEvent::PitchBend { range } => CSourceKind::PitchBend { range: *range },
        SourceEvent::ChannelPressure { range } => CSourceKind::ChannelPressure { range: *range },
        SourceEvent::PolyPressure { notes, range } => {
            CSourceKind::PolyPressure { notes: *notes, range: *range }
        }
        SourceEvent::SysEx { pattern } => match SysExPattern::parse(pattern) {
            Ok(parsed) => CSourceKind::SysEx { pattern: Some(parsed) },
            Err(error) => {
                errors.push(format!(
                    "Mapping \"{label}\" in profile \"{}\": invalid SysEx pattern \"{pattern}\" \
                     ({error}). It will not react until the pattern is fixed.",
                    profiles[pi].name
                ));
                CSourceKind::SysEx { pattern: None }
            }
        },
        SourceEvent::Clock { divisor } => CSourceKind::Clock { divisor: (*divisor).max(1) },
        SourceEvent::SongSelect { songs } => CSourceKind::SongSelect { songs: *songs },
        SourceEvent::SongPosition { range } => CSourceKind::SongPosition { range: *range },
        SourceEvent::System { message } => CSourceKind::System { message: *message },
    };
    // Hysteresis only means something for ranged, absolute values.
    let ranged = match &kind {
        CSourceKind::Cc { encoding, .. } => *encoding == ValueEncoding::Absolute,
        CSourceKind::Cc14 { .. }
        | CSourceKind::Param { .. }
        | CSourceKind::PitchBend { .. }
        | CSourceKind::ChannelPressure { .. }
        | CSourceKind::PolyPressure { .. }
        | CSourceKind::SongPosition { .. } => true,
        _ => false,
    };
    // Press-only buttons resend their "on" value: for these sources a repeated value presses.
    let presses_on_repeat = match &kind {
        CSourceKind::Cc { encoding, .. } => *encoding == ValueEncoding::Absolute,
        CSourceKind::Cc14 { .. }
        | CSourceKind::Param { .. }
        | CSourceKind::PitchBend { .. }
        | CSourceKind::ChannelPressure { .. }
        | CSourceKind::PolyPressure { .. } => true,
        _ => false,
    };
    // Debouncing filters press/release changes: notes, chords and ranged absolute values.
    let gated = ranged || matches!(kind, CSourceKind::Note { .. } | CSourceKind::Chord { .. });
    let source = CSource {
        device: mapping.source.device.as_deref().filter(|d| !d.is_empty()).map(Box::from),
        channel: mapping.source.channel,
        kind,
        hysteresis: if ranged { mapping.source.hysteresis } else { 0 },
        presses_on_repeat,
        debounce_ms: if gated { mapping.source.debounce_ms } else { 0 },
        ranged,
    };

    let behavior = match &mapping.behavior {
        Behavior::Trigger { on_press, on_release, repeat } => CBehavior::Trigger {
            on_press: act_opt(on_press),
            on_release: act_opt(on_release),
            repeat: *repeat,
        },
        Behavior::Hold { target, .. } | Behavior::Toggle { target } if target.is_empty() => {
            CBehavior::Inert
        }
        Behavior::Hold { target, auto_repeat } => {
            let pulse = match target {
                HoldTarget::Keys { keys } => {
                    Some(act(&Action::Keys { keys: keys.clone(), tap_ms: None }))
                }
                HoldTarget::MouseButton { button } => {
                    Some(act(&Action::MouseClick { button: *button, clicks: 1 }))
                }
                HoldTarget::Layer { .. } => None,
            };
            CBehavior::Hold { target: hold_target(target), auto_repeat: *auto_repeat, pulse }
        }
        Behavior::Toggle { target } => CBehavior::Toggle { target: hold_target(target) },
        Behavior::Cycle { actions, reset_after_ms } => CBehavior::Cycle {
            // An empty entry is skipped rather than spending a press on nothing.
            actions: actions.iter().filter(|a| !a.is_empty()).map(act).collect(),
            reset_after_ms: *reset_after_ms,
        },
        Behavior::TapHold { tap, hold, hold_target: target, hold_ms } => CBehavior::TapHold {
            tap: act_opt(tap),
            hold: act_opt(hold),
            hold_target: target.as_ref().filter(|t| !t.is_empty()).map(hold_target),
            hold_ms: *hold_ms,
        },
        Behavior::MultiTap { single, double, triple, window_ms, hold, hold_ms } => {
            let actions = [act_opt(single), act_opt(double), act_opt(triple)];
            let highest = actions.iter().rposition(Option::is_some).map_or(0, |i| i as u8 + 1);
            CBehavior::MultiTap {
                actions,
                highest,
                window_ms: *window_ms,
                hold: act_opt(hold),
                hold_ms: (*hold_ms).max(1),
            }
        }
        Behavior::Change { action, .. } if action.is_empty() => CBehavior::Inert,
        Behavior::Change { action, min_change } => {
            CBehavior::Change { action: act(action), min_change: *min_change }
        }
        Behavior::Step { up, down, step, max_burst, invert, fast } => CBehavior::Step {
            up: act_opt(up),
            down: act_opt(down),
            // A direction without a fast action (or with an empty one) keeps its normal steps.
            fast: fast
                .as_ref()
                .map(|fast| CFastStep {
                    ticks: u32::from(fast.ticks.max(2)),
                    up: act_opt(&fast.up),
                    down: act_opt(&fast.down),
                })
                .filter(|fast| fast.up.is_some() || fast.down.is_some()),
            step: (*step).max(1),
            max_burst: *max_burst,
            invert: *invert,
        },
        // Not built yet: the mapping loads and does nothing.
        Behavior::Shuttle { .. } => CBehavior::Inert,
    };

    for action in behavior_actions(&mapping.behavior) {
        if let Some(problem) = midi_problem(action) {
            errors.push(format!(
                "Mapping \"{label}\" in profile \"{}\": {problem}. That message will not be sent.",
                profiles[pi].name
            ));
        }
        if let Action::SwitchProfile { target: ProfileTarget::Profile { id } } = action
            && !action.is_empty()
            && !profiles.iter().any(|p| &*p.id == id)
        {
            errors.push(format!(
                "Mapping \"{label}\" in profile \"{}\" switches to a profile that does not exist (id \"{id}\").",
                profiles[pi].name
            ));
        }
    }

    let conditions = mapping
        .conditions
        .iter()
        .map(|c| match c {
            Condition::App { app, negate } => CCondition::App {
                rule: AppRule::new(app),
                negate: *negate,
                process: app.process.trim().to_string(),
                title: app.title.trim().to_string(),
            },
            Condition::Layer { layer, active } => {
                CCondition::Layer { layer: layer.clone(), active: *active }
            }
            Condition::NoteHeld { channel, note, held } => {
                CCondition::NoteHeld { channel: *channel, note: *note, held: *held }
            }
            Condition::ControlValue { channel, controller, range } => CCondition::ControlValue {
                channel: *channel,
                controller: *controller,
                range: *range,
            },
        })
        .collect();

    let feedback = mapping.feedback.as_ref().and_then(|fb| {
        let mut half = |which: &str, send: &MidiSend| match CSend::new(send) {
            Ok(send) => (!send.is_empty()).then_some(send),
            Err(error) => {
                errors.push(format!(
                    "Mapping \"{label}\" in profile \"{}\": its feedback's \"{which}\" message is \
                     not valid ({error}); it will not be sent.",
                    profiles[pi].name
                ));
                None
            }
        };
        let (on, off) = (half("on", &fb.on), half("off", &fb.off));
        let when = match &fb.when {
            FeedbackWhen::Engaged => CWhen::Engaged,
            FeedbackWhen::Layer { layer } => CWhen::Layer(Arc::from(layer.as_str())),
            FeedbackWhen::Profile { id } => CWhen::Profile(Arc::from(id.as_str())),
        };
        let device = fb.device.as_deref().map(Arc::from);
        (on.is_some() || off.is_some()).then_some(CFeedback { device, when, on, off })
    });

    CMapping {
        id: Arc::from(mapping.id.as_str()),
        profile: pi,
        profile_id: profiles[pi].id.clone(),
        def: mapping.clone(),
        source,
        behavior,
        conditions,
        rate_limit: mapping.rate_limit,
        feedback,
    }
}

/// Every action a behavior can fire.
fn behavior_actions(behavior: &Behavior) -> Vec<&Action> {
    match behavior {
        Behavior::Trigger { on_press, on_release, .. } => {
            on_press.iter().chain(on_release).collect()
        }
        Behavior::Hold { .. } | Behavior::Toggle { .. } => Vec::new(),
        Behavior::Cycle { actions, .. } => actions.iter().collect(),
        Behavior::TapHold { tap, hold, .. } => tap.iter().chain(hold).collect(),
        Behavior::MultiTap { single, double, triple, .. } => {
            single.iter().chain(double).chain(triple).collect()
        }
        Behavior::Change { action, .. } => vec![action],
        Behavior::Step { up, down, fast, .. } => up
            .iter()
            .chain(down)
            .chain(fast.iter().flat_map(|fast| fast.up.iter().chain(&fast.down)))
            .collect(),
        Behavior::Shuttle { up, down, .. } => up.iter().chain(down).collect(),
    }
}

fn hold_target(target: &HoldTarget) -> CHoldTarget {
    let (what, label) = match target {
        HoldTarget::Keys { keys } => (HoldWhat::Keys(Arc::from(keys.as_slice())), keys_label(keys)),
        HoldTarget::MouseButton { button } => {
            (HoldWhat::Button(*button), format!("{} button", button_name(*button)))
        }
        HoldTarget::Layer { layer } => {
            (HoldWhat::Layer(Arc::from(layer.as_str())), format!("layer \"{layer}\""))
        }
    };
    CHoldTarget {
        what,
        hold_label: format!("hold {label}"),
        release_label: format!("release {label}"),
    }
}

/// Compiles one action with the given settings (tap duration, stagger).
pub fn action(action: &Action, settings: &Settings, profiles: &[CProfile]) -> CAction {
    let stagger = settings.key_stagger_ms;
    let gap = stagger.max(MIN_GAP_MS);
    let mut dynamic = None;
    let (kind, class, label) = match action {
        Action::Keys { keys, tap_ms } => {
            let mut steps = Vec::new();
            push_tap(&mut steps, keys, tap_ms.unwrap_or(settings.tap_ms).max(1), stagger);
            (ActionKind::Output(steps), Class::Os, keys_label(keys))
        }
        Action::Text { text, char_delay_ms } => {
            let template = TextTemplate::parse(text);
            if !template.is_literal() {
                dynamic = Some(template.clone());
            }
            let steps = vec![Prim::Text { text: template, char_delay_ms: *char_delay_ms }];
            (ActionKind::Output(steps), Class::Os, type_label(text))
        }
        Action::MouseClick { button, clicks } => {
            let mut steps = Vec::new();
            push_click(&mut steps, *button, *clicks, settings.tap_ms, gap.max(CLICK_GAP_MS));
            (ActionKind::Output(steps), Class::Os, click_label(*button, *clicks))
        }
        Action::MouseScroll { dx, dy } => (
            ActionKind::Output(vec![Prim::Scroll { dx: *dx, dy: *dy }]),
            Class::Os,
            scroll_label(*dx, *dy),
        ),
        Action::MouseMove { x, y, absolute } => (
            ActionKind::Output(vec![Prim::Move { x: *x, y: *y, absolute: *absolute }]),
            Class::Os,
            move_label(*x, *y, *absolute),
        ),
        Action::Sequence { steps } => match sequence(steps, settings, gap) {
            Ok(prims) => (ActionKind::Output(prims), sequence_class(steps), sequence_label(steps)),
            Err(error) => (
                ActionKind::Unavailable(format!("its MIDI bytes cannot be sent: {error}")),
                sequence_class(steps),
                sequence_label(steps),
            ),
        },
        Action::SwitchProfile { target } => (
            match target {
                ProfileTarget::Profile { id } if !profiles.iter().any(|p| &*p.id == id) => {
                    ActionKind::Unavailable(format!("there is no profile with id \"{id}\""))
                }
                _ => ActionKind::SwitchProfile(target.clone()),
            },
            Class::Internal,
            match target {
                ProfileTarget::Next => "next profile".to_string(),
                ProfileTarget::Previous => "previous profile".to_string(),
                ProfileTarget::ByValue => "profile by value".to_string(),
                ProfileTarget::Profile { id } => {
                    let name = profiles.iter().find(|p| &*p.id == id).map_or(id, |p| &p.name);
                    format!("switch to profile \"{name}\"")
                }
            },
        ),
        Action::Layer { layer, op } => (
            ActionKind::Layer { layer: Arc::from(layer.as_str()), op: *op },
            Class::Internal,
            match op {
                LayerOp::On => format!("layer \"{layer}\" on"),
                LayerOp::Off => format!("layer \"{layer}\" off"),
                LayerOp::Toggle => format!("toggle layer \"{layer}\""),
            },
        ),
        Action::Engine { op } => (
            ActionKind::Engine(*op),
            Class::Engine,
            match op {
                EngineOp::Pause => "pause output",
                EngineOp::Resume => "resume output",
                EngineOp::TogglePause => "toggle pause",
                EngineOp::ReleaseAll => "release all",
            }
            .to_string(),
        ),
        Action::Run { command, args, working_dir } => {
            let mut label = format!("run {command}");
            for arg in args {
                label.push(' ');
                label.push_str(arg);
            }
            (
                ActionKind::Launch(Launch::Run {
                    command: command.clone(),
                    args: args.clone(),
                    working_dir: working_dir.clone(),
                }),
                Class::Os,
                shorten(&label, 60).into_owned(),
            )
        }
        Action::Open { target } => (
            ActionKind::Launch(Launch::Open { target: target.clone() }),
            Class::Os,
            format!("open {}", shorten(target, 60)),
        ),
        // Not held back while the app's window is in front (lighting a pad is not typing into
        // the editor), but stopped by pause and learning like layers and profile switches.
        Action::Midi { device, message } => (
            match CSend::new(message) {
                Ok(send) => {
                    ActionKind::Midi(CMidi { device: device.as_deref().map(Arc::from), send })
                }
                Err(error) => {
                    ActionKind::Unavailable(format!("the MIDI bytes cannot be sent: {error}"))
                }
            },
            Class::Internal,
            "send MIDI".to_string(),
        ),
    };
    let est_ms = match &kind {
        ActionKind::Output(steps) => steps
            .iter()
            .map(|step| match step {
                Prim::Wait(ms) => *ms,
                Prim::Text { text, char_delay_ms } => {
                    char_delay_ms.saturating_mul(text.approx_chars().saturating_sub(1) as u32)
                }
                _ => 0,
            })
            .fold(0u32, u32::saturating_add),
        _ => 0,
    };
    Arc::new(CompiledAction { kind, class, est_ms, label, dynamic })
}

/// Downs in order (with stagger), hold for `tap_ms`, ups in reverse order (with stagger).
fn push_tap(steps: &mut Vec<Prim>, keys: &[Key], tap_ms: u32, stagger: u32) {
    if keys.is_empty() {
        return;
    }
    for (i, key) in keys.iter().enumerate() {
        if i > 0 && stagger > 0 {
            steps.push(Prim::Wait(stagger));
        }
        steps.push(Prim::Down(*key));
    }
    steps.push(Prim::Wait(tap_ms));
    for (i, key) in keys.iter().rev().enumerate() {
        if i > 0 && stagger > 0 {
            steps.push(Prim::Wait(stagger));
        }
        steps.push(Prim::Up(*key));
    }
}

fn push_click(steps: &mut Vec<Prim>, button: MouseButton, clicks: u8, hold_ms: u32, gap: u32) {
    for i in 0..clicks.max(1) {
        if i > 0 {
            steps.push(Prim::Wait(gap));
        }
        steps.push(Prim::ButtonDown(button));
        steps.push(Prim::Wait(hold_ms.max(1)));
        steps.push(Prim::ButtonUp(button));
    }
}

/// The steps of a macro; fails on raw MIDI bytes that are not MIDI.
fn sequence(steps: &[SequenceStep], settings: &Settings, gap: u32) -> Result<Vec<Prim>, String> {
    let mut out = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        // Separate consecutive taps so each one registers, unless a delay follows anyway.
        let separate = !matches!(steps.get(i + 1), None | Some(SequenceStep::Delay { .. }));
        match step {
            SequenceStep::Midi { device, message } => out.push(Prim::Midi(CMidi {
                device: device.as_deref().map(Arc::from),
                send: CSend::new(message)?,
            })),
            SequenceStep::Keys { keys, tap_ms } => {
                let tap_ms = tap_ms.unwrap_or(settings.tap_ms).max(1);
                push_tap(&mut out, keys, tap_ms, settings.key_stagger_ms);
                if separate && !keys.is_empty() {
                    out.push(Prim::Wait(gap));
                }
            }
            SequenceStep::KeyDown { key } => out.push(Prim::Down(*key)),
            SequenceStep::KeyUp { key } => out.push(Prim::Up(*key)),
            SequenceStep::Text { text } => {
                out.push(Prim::Text { text: TextTemplate::parse(text), char_delay_ms: 0 })
            }
            SequenceStep::Delay { ms } => {
                if *ms > 0 {
                    out.push(Prim::Wait(*ms));
                }
            }
            SequenceStep::MouseClick { button, clicks } => {
                push_click(&mut out, *button, *clicks, settings.tap_ms, gap.max(CLICK_GAP_MS));
                if separate {
                    out.push(Prim::Wait(gap));
                }
            }
            SequenceStep::MouseDown { button } => out.push(Prim::ButtonDown(*button)),
            SequenceStep::MouseUp { button } => out.push(Prim::ButtonUp(*button)),
            SequenceStep::MouseScroll { dx, dy } => out.push(Prim::Scroll { dx: *dx, dy: *dy }),
            SequenceStep::MouseMove { x, y, absolute } => {
                out.push(Prim::Move { x: *x, y: *y, absolute: *absolute })
            }
        }
    }
    Ok(out)
}

/// A macro that only sends MIDI (and waits) is gated like MIDI, not like typing.
fn sequence_class(steps: &[SequenceStep]) -> Class {
    let midi_only = steps
        .iter()
        .all(|step| matches!(step, SequenceStep::Midi { .. } | SequenceStep::Delay { .. }));
    if midi_only && !steps.is_empty() { Class::Internal } else { Class::Os }
}

/// What is wrong with the raw MIDI bytes of an action (or of its macro steps), if anything.
fn midi_problem(action: &Action) -> Option<String> {
    let raw = |message: &MidiSend| match message {
        MidiSend::Raw { bytes } => parse_raw(bytes).err().map(|error| {
            format!("the MIDI bytes \"{}\" are not valid ({error})", shorten(bytes.trim(), 40))
        }),
        _ => None,
    };
    match action {
        Action::Midi { message, .. } => raw(message),
        Action::Sequence { steps } => steps.iter().find_map(|step| match step {
            SequenceStep::Midi { message, .. } => raw(message),
            _ => None,
        }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------------------------

pub fn keys_label(keys: &[Key]) -> String {
    if keys.is_empty() {
        return "(no keys)".into();
    }
    keys.iter().map(|k| k.label()).collect::<Vec<_>>().join("+")
}

pub fn button_name(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "Left",
        MouseButton::Right => "Right",
        MouseButton::Middle => "Middle",
        MouseButton::Back => "Back",
        MouseButton::Forward => "Forward",
    }
}

fn click_label(button: MouseButton, clicks: u8) -> String {
    let name = button_name(button);
    match clicks {
        0 | 1 => format!("click {name}"),
        2 => format!("double-click {name}"),
        3 => format!("triple-click {name}"),
        n => format!("click {name} x{n}"),
    }
}

fn scroll_label(dx: i32, dy: i32) -> String {
    let mut parts = Vec::new();
    if dy > 0 {
        parts.push(format!("up {dy}"));
    } else if dy < 0 {
        parts.push(format!("down {}", -(dy as i64)));
    }
    if dx > 0 {
        parts.push(format!("right {dx}"));
    } else if dx < 0 {
        parts.push(format!("left {}", -(dx as i64)));
    }
    if parts.is_empty() { "scroll 0".into() } else { format!("scroll {}", parts.join(", ")) }
}

fn move_label(x: i32, y: i32, absolute: bool) -> String {
    if absolute {
        format!("move pointer to ({x}, {y})")
    } else {
        format!("move pointer by ({x}, {y})")
    }
}

fn sequence_label(steps: &[SequenceStep]) -> String {
    let parts: Vec<String> = steps
        .iter()
        .take(4)
        .map(|step| match step {
            SequenceStep::Midi { .. } => "send MIDI".to_string(),
            SequenceStep::Keys { keys, .. } => keys_label(keys),
            SequenceStep::KeyDown { key } => format!("press {}", key.label()),
            SequenceStep::KeyUp { key } => format!("release {}", key.label()),
            SequenceStep::Text { text } => type_label(text),
            SequenceStep::Delay { ms } => format!("wait {ms} ms"),
            SequenceStep::MouseClick { button, clicks } => click_label(*button, *clicks),
            SequenceStep::MouseDown { button } => format!("press {} button", button_name(*button)),
            SequenceStep::MouseUp { button } => {
                format!("release {} button", button_name(*button))
            }
            SequenceStep::MouseScroll { dx, dy } => scroll_label(*dx, *dy),
            SequenceStep::MouseMove { x, y, absolute } => move_label(*x, *y, *absolute),
        })
        .collect();
    let mut label = format!("macro: {}", parts.join(", "));
    if steps.len() > 4 {
        label.push_str(", …");
    }
    if steps.is_empty() {
        label = "macro (empty)".into();
    }
    shorten(&label, 80).into_owned()
}

// ---------------------------------------------------------------------------------------------
// Index
// ---------------------------------------------------------------------------------------------

/// Live mappings by the message kind that can feed them (indices into
/// [`CompiledConfig::mappings`]). Rebuilt whenever the set of live mappings changes.
pub struct SourceIndex {
    pub note: Vec<usize>,
    pub chord: Vec<usize>,
    pub poly_pressure: Vec<usize>,
    pub channel_pressure: Vec<usize>,
    pub pitch_bend: Vec<usize>,
    pub program: Vec<usize>,
    /// Per controller number.
    pub cc: Vec<Vec<usize>>,
    /// Per MSB controller number (0..=31).
    pub cc14: Vec<Vec<usize>>,
    pub nrpn: Vec<usize>,
    pub rpn: Vec<usize>,
    pub sysex: Vec<usize>,
    pub clock: Vec<usize>,
    pub song_select: Vec<usize>,
    pub song_position: Vec<usize>,
    /// Per [`system_index`].
    pub system: [Vec<usize>; 6],
}

pub fn system_index(message: SystemMessage) -> usize {
    match message {
        SystemMessage::Start => 0,
        SystemMessage::Continue => 1,
        SystemMessage::Stop => 2,
        SystemMessage::TuneRequest => 3,
        SystemMessage::Reset => 4,
        SystemMessage::ActiveSensing => 5,
    }
}

impl SourceIndex {
    pub fn build(mappings: &[CMapping], live: impl Fn(usize) -> bool) -> Self {
        let mut index = SourceIndex {
            note: Vec::new(),
            chord: Vec::new(),
            poly_pressure: Vec::new(),
            channel_pressure: Vec::new(),
            pitch_bend: Vec::new(),
            program: Vec::new(),
            cc: vec![Vec::new(); 128],
            cc14: vec![Vec::new(); 32],
            nrpn: Vec::new(),
            rpn: Vec::new(),
            sysex: Vec::new(),
            clock: Vec::new(),
            song_select: Vec::new(),
            song_position: Vec::new(),
            system: Default::default(),
        };
        for (i, mapping) in mappings.iter().enumerate() {
            if !live(i) {
                continue;
            }
            match &mapping.source.kind {
                CSourceKind::Note { .. } => index.note.push(i),
                CSourceKind::Chord { mask, .. } => {
                    if *mask != 0 {
                        index.chord.push(i);
                    }
                }
                CSourceKind::Cc { controller, .. } => {
                    index.cc[usize::from(*controller & 127)].push(i)
                }
                CSourceKind::Cc14 { controller, .. } => {
                    index.cc14[usize::from(*controller & 31)].push(i)
                }
                CSourceKind::Param { kind: ParameterKind::Nrpn, .. } => index.nrpn.push(i),
                CSourceKind::Param { kind: ParameterKind::Rpn, .. } => index.rpn.push(i),
                CSourceKind::Program { .. } => index.program.push(i),
                CSourceKind::PitchBend { .. } => index.pitch_bend.push(i),
                CSourceKind::ChannelPressure { .. } => index.channel_pressure.push(i),
                CSourceKind::PolyPressure { .. } => index.poly_pressure.push(i),
                CSourceKind::SysEx { .. } => index.sysex.push(i),
                CSourceKind::Clock { .. } => index.clock.push(i),
                CSourceKind::SongSelect { .. } => index.song_select.push(i),
                CSourceKind::SongPosition { .. } => index.song_position.push(i),
                CSourceKind::System { message } => index.system[system_index(*message)].push(i),
            }
        }
        index
    }
}
