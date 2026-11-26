// Factories, defaults and metadata for the config model (the generated types live in
// src/bindings). Everything that decides "what does a fresh X look like" is here.

import type { Action } from "@/bindings/Action";
import type { Behavior } from "@/bindings/Behavior";
import type { Condition } from "@/bindings/Condition";
import type { Config } from "@/bindings/Config";
import type { HoldTarget } from "@/bindings/HoldTarget";
import type { Mapping } from "@/bindings/Mapping";
import type { Profile } from "@/bindings/Profile";
import type { RateLimit } from "@/bindings/RateLimit";
import type { SequenceStep } from "@/bindings/SequenceStep";
import type { Source } from "@/bindings/Source";
import type { SourceEvent } from "@/bindings/SourceEvent";
import type { ValueRange } from "@/bindings/ValueRange";

import { newId } from "./utils";

export type SourceType = SourceEvent["type"];
export type BehaviorType = Behavior["type"];
export type ActionType = Action["type"];
export type StepType = SequenceStep["type"];
export type ConditionType = Condition["type"];

// ---------------------------------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------------------------------

export interface SourceTypeInfo {
  type: SourceType;
  label: string;
  hint: string;
  /** Has a MIDI channel. */
  channel: boolean;
}

export const SOURCE_TYPES: SourceTypeInfo[] = [
  { type: "note", label: "Note", hint: "Keys and pads", channel: true },
  { type: "chord", label: "Chord", hint: "Several notes held together", channel: true },
  { type: "control_change", label: "Control Change", hint: "Knobs, faders, pedals, buttons", channel: true },
  { type: "program_change", label: "Program Change", hint: "Patch / preset buttons", channel: true },
  { type: "pitch_bend", label: "Pitch Bend", hint: "The pitch wheel", channel: true },
  { type: "channel_pressure", label: "Channel Pressure", hint: "Aftertouch for the whole channel", channel: true },
  { type: "poly_pressure", label: "Poly Pressure", hint: "Aftertouch per key", channel: true },
  { type: "control_change14", label: "14-bit CC", hint: "High-resolution MSB + LSB pair", channel: true },
  { type: "nrpn", label: "NRPN", hint: "Non-registered parameter (CC 99 / 98 + data)", channel: true },
  { type: "rpn", label: "RPN", hint: "Registered parameter (CC 101 / 100 + data)", channel: true },
  { type: "sys_ex", label: "SysEx", hint: "System exclusive pattern, incl. MMC transport", channel: false },
  { type: "system", label: "Transport", hint: "Start, Stop, Continue and friends", channel: false },
  { type: "clock", label: "Clock", hint: "Every N ticks of MIDI clock", channel: false },
  { type: "song_select", label: "Song Select", hint: "Song number", channel: false },
  { type: "song_position", label: "Song Position", hint: "Song position pointer", channel: false },
];

export function sourceTypeInfo(type: SourceType): SourceTypeInfo {
  return SOURCE_TYPES.find((info) => info.type === type)!;
}

export const FULL7: ValueRange = { min: 0, max: 127 };
export const FULL14: ValueRange = { min: 0, max: 16383 };
/** "Pressed" half of a 7-bit control. */
export const UPPER7: ValueRange = { min: 64, max: 127 };

export function defaultSourceEvent(type: SourceType): SourceEvent {
  switch (type) {
    case "note":
      return { type, notes: { min: 60, max: 60 }, velocity: { min: 1, max: 127 } };
    case "chord":
      return { type, notes: [60, 64, 67], window_ms: 0 };
    case "control_change":
      return { type, controller: 1, range: { ...FULL7 }, encoding: "absolute" };
    case "control_change14":
      return { type, controller: 1, range: { ...FULL14 } };
    case "nrpn":
    case "rpn":
      return { type, number: 0, range: { ...FULL7 }, hires: false };
    case "program_change":
      return { type, programs: { min: 0, max: 0 } };
    case "pitch_bend":
      return { type, range: { ...FULL14 } };
    case "channel_pressure":
      return { type, range: { ...FULL7 } };
    case "poly_pressure":
      return { type, notes: { min: 60, max: 60 }, range: { ...FULL7 } };
    case "sys_ex":
      return { type, pattern: "F0 7F ?? 06 02 F7" };
    case "clock":
      return { type, divisor: 24 };
    case "song_select":
      return { type, songs: { min: 0, max: 0 } };
    case "song_position":
      return { type, range: { ...FULL14 } };
    case "system":
      return { type, message: "start" };
  }
}

/**
 * How a source behaves physically, which decides the behaviors worth offering:
 * - `button`: has a press and a release (notes; controls restricted to a "pressed" range).
 * - `continuous`: a value that moves (knobs, faders, wheels, pressure).
 * - `relative`: an endless encoder sending ticks.
 * - `pulse`: happens at an instant (program change, SysEx, transport, clock).
 */
export type SourceShape = "button" | "continuous" | "relative" | "pulse";

/** The maximum value of the source's range (127 or 16383), or `null` when it has no range. */
export function rangeLimit(event: SourceEvent): number | null {
  switch (event.type) {
    case "control_change":
    case "channel_pressure":
    case "poly_pressure":
      return 127;
    case "nrpn":
    case "rpn":
      return event.hires ? 16383 : 127;
    case "control_change14":
    case "pitch_bend":
    case "song_position":
      return 16383;
    default:
      return null;
  }
}

export function sourceRange(event: SourceEvent): ValueRange | null {
  return "range" in event ? event.range : null;
}

export function isFullRange(event: SourceEvent): boolean {
  const limit = rangeLimit(event);
  const range = sourceRange(event);
  return !range || limit === null || (range.min === 0 && range.max === limit);
}

/** A control change source reshaped as a knob, a button or an endless encoder. */
export function ccWithShape(
  event: Extract<SourceEvent, { type: "control_change" }>,
  shape: "continuous" | "button" | "relative",
): SourceEvent {
  return {
    ...event,
    encoding:
      shape === "relative"
        ? event.encoding === "absolute"
          ? "relative_twos_complement"
          : event.encoding
        : "absolute",
    range: shape === "button" ? { ...UPPER7 } : { ...FULL7 },
  };
}

export function sourceShape(event: SourceEvent): SourceShape {
  switch (event.type) {
    case "note":
    case "chord":
      return "button";
    case "control_change":
      if (event.encoding !== "absolute") return "relative";
      return isFullRange(event) ? "continuous" : "button";
    case "control_change14":
    case "nrpn":
    case "rpn":
    case "pitch_bend":
    case "channel_pressure":
    case "poly_pressure":
    case "song_position":
      return isFullRange(event) ? "continuous" : "button";
    case "program_change":
    case "sys_ex":
    case "clock":
    case "song_select":
    case "system":
      return "pulse";
  }
}

// ---------------------------------------------------------------------------------------------
// Behaviors
// ---------------------------------------------------------------------------------------------

export interface BehaviorTypeInfo {
  type: BehaviorType;
  label: string;
  /** One line shown in the picker. */
  hint: string;
  /** Source shapes this behavior is a natural fit for (others are still allowed). */
  fits: SourceShape[];
}

export const BEHAVIOR_TYPES: BehaviorTypeInfo[] = [
  {
    type: "trigger",
    label: "Press",
    hint: "Fire once when pressed (and, optionally, when released)",
    fits: ["button", "pulse", "relative"],
  },
  {
    type: "hold",
    label: "Hold",
    hint: "Keep the keys down for as long as it is held",
    fits: ["button"],
  },
  {
    type: "toggle",
    label: "Toggle",
    hint: "First press holds the keys down, the next press lets go",
    fits: ["button", "pulse"],
  },
  {
    type: "change",
    label: "On change",
    hint: "Fire every time the value moves",
    fits: ["continuous", "relative"],
  },
  {
    type: "step",
    label: "Up / Down",
    hint: "One action when the value goes up, another when it goes down",
    fits: ["continuous", "relative"],
  },
  {
    type: "tap_hold",
    label: "Tap or hold",
    hint: "One action for a quick tap, another for a long press",
    fits: ["button"],
  },
  {
    type: "multi_tap",
    label: "Multi-tap",
    hint: "Different actions for single, double and triple press — and a long press",
    fits: ["button", "pulse"],
  },
  {
    type: "cycle",
    label: "Cycle",
    hint: "Each press fires the next action in a list",
    fits: ["button", "pulse"],
  },
];

export function behaviorTypeInfo(type: BehaviorType): BehaviorTypeInfo {
  return BEHAVIOR_TYPES.find((info) => info.type === type)!;
}

export function emptyKeys(): Action {
  return { type: "keys", keys: [], tap_ms: null };
}

/** The first plain action found inside a behavior (used to carry the user's keys across changes). */
export function primaryAction(behavior: Behavior): Action | null {
  switch (behavior.type) {
    case "trigger":
      return behavior.on_press ?? behavior.on_release;
    case "hold":
    case "toggle":
      return targetToAction(behavior.target);
    case "cycle":
      return behavior.actions[0] ?? null;
    case "tap_hold":
      return behavior.tap ?? behavior.hold ?? (behavior.hold_target ? targetToAction(behavior.hold_target) : null);
    case "multi_tap":
      return behavior.single ?? behavior.double ?? behavior.triple ?? behavior.hold;
    case "change":
      return behavior.action;
    case "step":
      return behavior.up ?? behavior.down;
  }
}

export function targetToAction(target: HoldTarget): Action {
  switch (target.type) {
    case "keys":
      return { type: "keys", keys: target.keys, tap_ms: null };
    case "mouse_button":
      return { type: "mouse_click", button: target.button, clicks: 1 };
    case "layer":
      return { type: "layer", layer: target.layer, op: "toggle" };
  }
}

export function actionToTarget(action: Action | null): HoldTarget {
  if (action?.type === "keys") return { type: "keys", keys: action.keys };
  if (action?.type === "mouse_click") return { type: "mouse_button", button: action.button };
  if (action?.type === "layer") return { type: "layer", layer: action.layer };
  return { type: "keys", keys: [] };
}

/** A fresh behavior of `type`, carrying over the user's existing action where it makes sense. */
export function defaultBehavior(type: BehaviorType, previous?: Behavior): Behavior {
  const carried = previous ? primaryAction(previous) : null;
  const action = carried ?? emptyKeys();
  switch (type) {
    case "trigger":
      return { type, on_press: action, on_release: null, repeat: null };
    case "hold":
      return { type, target: actionToTarget(carried), auto_repeat: true };
    case "toggle":
      return { type, target: actionToTarget(carried) };
    case "cycle":
      return { type, actions: [action, emptyKeys()], reset_after_ms: null };
    case "tap_hold":
      return { type, tap: action, hold: emptyKeys(), hold_target: null, hold_ms: 350 };
    case "multi_tap":
      return { type, single: action, double: emptyKeys(), triple: null, window_ms: 300, hold: null, hold_ms: 500 };
    case "change":
      return { type, action };
    case "step":
      return { type, up: action, down: emptyKeys(), step: 1, max_burst: 8, invert: false, fast: null };
  }
}

/** The behavior a freshly learned source should start with. */
export function suggestedBehavior(shape: SourceShape): BehaviorType {
  switch (shape) {
    case "button":
    case "pulse":
      return "trigger";
    case "continuous":
      return "change";
    case "relative":
      return "step";
  }
}

/** Behaviors that honor `rate_limit` (see `Mapping::rate_limit` in the Rust model). */
export function supportsRateLimit(type: BehaviorType): boolean {
  return type === "trigger" || type === "cycle" || type === "change" || type === "step";
}

// ---------------------------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------------------------

export interface ActionTypeInfo {
  type: ActionType;
  label: string;
  hint: string;
  group: "Keyboard" | "Mouse" | "App" | "System";
}

export const ACTION_TYPES: ActionTypeInfo[] = [
  { type: "keys", label: "Key / shortcut", hint: "Tap a key or a combination", group: "Keyboard" },
  { type: "text", label: "Type text", hint: "Type a piece of text", group: "Keyboard" },
  { type: "sequence", label: "Macro", hint: "A sequence of keys, text, clicks and pauses", group: "Keyboard" },
  { type: "mouse_click", label: "Mouse click", hint: "Click a mouse button", group: "Mouse" },
  { type: "mouse_scroll", label: "Scroll", hint: "Turn the scroll wheel", group: "Mouse" },
  { type: "mouse_move", label: "Move pointer", hint: "Move the mouse pointer", group: "Mouse" },
  { type: "switch_profile", label: "Switch profile", hint: "Activate another profile", group: "App" },
  { type: "layer", label: "Layer", hint: "Turn a layer on or off", group: "App" },
  { type: "engine", label: "Pause / resume", hint: "Pause output, resume, or release everything", group: "App" },
  { type: "run", label: "Run program", hint: "Launch an application or script", group: "System" },
  { type: "open", label: "Open file or URL", hint: "Open with the default application", group: "System" },
];

export function actionTypeInfo(type: ActionType): ActionTypeInfo {
  return ACTION_TYPES.find((info) => info.type === type)!;
}

export function defaultAction(type: ActionType, profileId = ""): Action {
  switch (type) {
    case "keys":
      return emptyKeys();
    case "text":
      return { type, text: "", char_delay_ms: 0 };
    case "mouse_click":
      return { type, button: "left", clicks: 1 };
    case "mouse_scroll":
      return { type, dx: 0, dy: 1 };
    case "mouse_move":
      return { type, x: 0, y: 0, absolute: false };
    case "sequence":
      return { type, steps: [] };
    case "switch_profile":
      return profileId ? { type, target: { type: "profile", id: profileId } } : { type, target: { type: "next" } };
    case "layer":
      return { type, layer: "shift", op: "toggle" };
    case "engine":
      return { type, op: "toggle_pause" };
    case "run":
      return { type, command: "", args: [], working_dir: "" };
    case "open":
      return { type, target: "" };
  }
}

export const STEP_TYPES: { type: StepType; label: string }[] = [
  { type: "keys", label: "Tap keys" },
  { type: "text", label: "Type text" },
  { type: "delay", label: "Wait" },
  { type: "key_down", label: "Key down" },
  { type: "key_up", label: "Key up" },
  { type: "mouse_click", label: "Mouse click" },
  { type: "mouse_down", label: "Mouse button down" },
  { type: "mouse_up", label: "Mouse button up" },
  { type: "mouse_scroll", label: "Scroll" },
  { type: "mouse_move", label: "Move pointer" },
];

export function defaultStep(type: StepType): SequenceStep {
  switch (type) {
    case "keys":
      return { type, keys: [], tap_ms: null };
    case "key_down":
    case "key_up":
      return { type, key: "ShiftLeft" };
    case "text":
      return { type, text: "" };
    case "delay":
      return { type, ms: 100 };
    case "mouse_click":
      return { type, button: "left", clicks: 1 };
    case "mouse_down":
    case "mouse_up":
      return { type, button: "left" };
    case "mouse_scroll":
      return { type, dx: 0, dy: 1 };
    case "mouse_move":
      return { type, x: 0, y: 0, absolute: false };
  }
}

/** Whether an action is "empty" (nothing configured yet). */
export function isEmptyAction(action: Action | null): boolean {
  if (!action) return true;
  switch (action.type) {
    case "keys":
      return action.keys.length === 0;
    case "text":
      return action.text.length === 0;
    case "sequence":
      return action.steps.length === 0;
    case "run":
      return action.command.trim().length === 0;
    case "open":
      return action.target.trim().length === 0;
    case "mouse_scroll":
      return action.dx === 0 && action.dy === 0;
    default:
      return false;
  }
}

// ---------------------------------------------------------------------------------------------
// Conditions / rate limit
// ---------------------------------------------------------------------------------------------

export const CONDITION_TYPES: { type: ConditionType; label: string; hint: string }[] = [
  { type: "app", label: "Application", hint: "Only while a certain app has focus" },
  { type: "layer", label: "Layer", hint: "Only while a layer is on (or off)" },
  { type: "note_held", label: "Note held", hint: "Only while another key / pad is held down" },
  { type: "control_value", label: "Control position", hint: "Only while a knob or switch is in a range" },
];

export function defaultCondition(type: ConditionType): Condition {
  switch (type) {
    case "app":
      return { type, app: { process: "", title: "" }, negate: false };
    case "layer":
      return { type, layer: "shift", active: true };
    case "note_held":
      return { type, channel: null, note: 36, held: true };
    case "control_value":
      return { type, channel: null, controller: 1, range: { ...UPPER7 } };
  }
}

export function defaultRateLimit(): RateLimit {
  return { mode: "throttle", interval_ms: 500, trailing: false };
}

// ---------------------------------------------------------------------------------------------
// Mappings / profiles
// ---------------------------------------------------------------------------------------------

export function newSource(event: SourceEvent = defaultSourceEvent("note")): Source {
  return { device: null, channel: null, event, hysteresis: 0, debounce_ms: 0 };
}

export function newMapping(source: Source = newSource(), behavior?: Behavior): Mapping {
  return {
    id: newId("map"),
    name: "",
    enabled: true,
    source,
    behavior: behavior ?? defaultBehavior(suggestedBehavior(sourceShape(source.event))),
    conditions: [],
    rate_limit: null,
  };
}

export function duplicateMapping(mapping: Mapping): Mapping {
  return { ...structuredClone(mapping), id: newId("map") };
}

export function newProfile(name: string): Profile {
  return { id: newId("profile"), name, always_on: false, apps: [], mappings: [] };
}

export function duplicateProfile(profile: Profile, name: string): Profile {
  const copy = structuredClone(profile);
  return {
    ...copy,
    id: newId("profile"),
    name,
    mappings: copy.mappings.map((mapping) => ({ ...mapping, id: newId("map") })),
  };
}

export function findProfile(config: Config, id: string | null): Profile | null {
  return config.profiles.find((profile) => profile.id === id) ?? null;
}

/** Every layer name mentioned anywhere in the config (for suggestions). */
export function knownLayers(config: Config): string[] {
  const names = new Set<string>();
  const visitAction = (action: Action | null) => {
    if (action?.type === "layer") names.add(action.layer);
  };
  const visitTarget = (target: HoldTarget | null) => {
    if (target?.type === "layer") names.add(target.layer);
  };
  for (const profile of config.profiles) {
    for (const mapping of profile.mappings) {
      for (const condition of mapping.conditions) {
        if (condition.type === "layer") names.add(condition.layer);
      }
      const b = mapping.behavior;
      switch (b.type) {
        case "trigger":
          visitAction(b.on_press);
          visitAction(b.on_release);
          break;
        case "hold":
        case "toggle":
          visitTarget(b.target);
          break;
        case "cycle":
          b.actions.forEach(visitAction);
          break;
        case "tap_hold":
          visitAction(b.tap);
          visitAction(b.hold);
          visitTarget(b.hold_target);
          break;
        case "multi_tap":
          visitAction(b.single);
          visitAction(b.double);
          visitAction(b.triple);
          visitAction(b.hold);
          break;
        case "change":
          visitAction(b.action);
          break;
        case "step":
          visitAction(b.up);
          visitAction(b.down);
          visitAction(b.fast?.up ?? null);
          visitAction(b.fast?.down ?? null);
          break;
      }
    }
  }
  return Array.from(names).filter(Boolean).sort();
}
