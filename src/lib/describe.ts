// Plain-language descriptions of config objects. The mapping list, tooltips and the monitor all
// speak through these so the wording stays consistent.

import type { Action } from "@/bindings/Action";
import type { Condition } from "@/bindings/Condition";
import type { Config } from "@/bindings/Config";
import type { HoldTarget } from "@/bindings/HoldTarget";
import type { Mapping } from "@/bindings/Mapping";
import type { RateLimit } from "@/bindings/RateLimit";
import type { SequenceStep } from "@/bindings/SequenceStep";
import type { Source } from "@/bindings/Source";
import type { SourceEvent } from "@/bindings/SourceEvent";
import type { ValueRange } from "@/bindings/ValueRange";

import { comboText, keyLabel, mouseButtonLabel } from "./keys";
import { ccName, noteName, rpnName } from "./midi";
import { isFullRange, rangeLimit } from "./model";
import { formatMs, plural } from "./utils";

export function rangeText(range: ValueRange, limit: number): string {
  if (range.min === range.max) return `= ${range.min}`;
  if (range.min <= 0 && range.max >= limit) return "any value";
  if (range.max >= limit) return `≥ ${range.min}`;
  if (range.min <= 0) return `≤ ${range.max}`;
  return `${range.min}–${range.max}`;
}

function noteRangeText(range: ValueRange, octave: number): string {
  if (range.min === range.max) return noteName(range.min, octave);
  if (range.min === 0 && range.max === 127) return "any note";
  return `${noteName(range.min, octave)}–${noteName(range.max, octave)}`;
}

/** The headline of a source: "C4", "CC 20", "Program 5", "Pitch Bend"… */
export function sourceTitle(event: SourceEvent, octave = 4): string {
  switch (event.type) {
    case "note":
      return noteRangeText(event.notes, octave);
    case "chord":
      return event.notes.length ? event.notes.map((note) => noteName(note, octave)).join(" + ") : "Empty chord";
    case "control_change":
      return `CC ${event.controller}`;
    case "control_change14":
      return `CC ${event.controller}/${event.controller + 32}`;
    case "nrpn":
      return `NRPN ${event.number}`;
    case "rpn":
      return `RPN ${event.number}`;
    case "program_change":
      return event.programs.min === event.programs.max
        ? `Program ${event.programs.min}`
        : event.programs.min === 0 && event.programs.max === 127
          ? "Any program"
          : `Program ${event.programs.min}–${event.programs.max}`;
    case "pitch_bend":
      return "Pitch Bend";
    case "channel_pressure":
      return "Pressure";
    case "poly_pressure":
      return `Pressure ${noteRangeText(event.notes, octave)}`;
    case "sys_ex":
      return "SysEx";
    case "clock":
      return event.divisor === 24
        ? "Every beat"
        : event.divisor === 96
          ? "Every bar"
          : `Every ${plural(event.divisor, "tick")}`;
    case "song_select":
      return event.songs.min === event.songs.max ? `Song ${event.songs.min}` : "Song select";
    case "song_position":
      return "Song Position";
    case "system":
      return {
        start: "Start",
        continue: "Continue",
        stop: "Stop",
        tune_request: "Tune Request",
        reset: "System Reset",
        active_sensing: "Active Sensing",
      }[event.message];
  }
}

/** A conventional name for the source if it has one ("Sustain Pedal", "Mod Wheel"). */
export function sourceNickname(event: SourceEvent): string {
  if (event.type === "control_change") return ccName(event.controller);
  if (event.type === "rpn") return rpnName(event.number);
  return "";
}

/** The small-print qualifiers of a source: channel, ranges, device, encoding. */
export function sourceQualifiers(source: Source, config: Config | null): string[] {
  const parts: string[] = [];
  const event = source.event;
  if (source.channel !== null) parts.push(`ch ${source.channel}`);
  switch (event.type) {
    case "note":
      if (event.velocity.min > 1 || event.velocity.max < 127) {
        parts.push(`vel ${rangeText(event.velocity, 127).replace("= ", "")}`);
      }
      break;
    case "control_change":
      if (event.encoding !== "absolute") parts.push("relative");
      else if (!isFullRange(event)) parts.push(rangeText(event.range, 127));
      break;
    case "sys_ex":
      parts.push(event.pattern.length > 26 ? `${event.pattern.slice(0, 24)}…` : event.pattern);
      break;
    case "nrpn":
    case "rpn":
      if (event.hires) parts.push("14-bit");
      if (!isFullRange(event)) parts.push(rangeText(event.range, rangeLimit(event)!));
      break;
    default: {
      const limit = rangeLimit(event);
      if (limit !== null && "range" in event && !isFullRange(event)) {
        parts.push(
          event.type === "pitch_bend"
            ? bendRangeText(event.range)
            : rangeText(event.range, limit),
        );
      }
    }
  }
  if (event.type === "chord" && event.window_ms > 0) parts.push(`within ${event.window_ms} ms`);
  if (source.hysteresis > 0 && !isFullRange(event)) parts.push(`dead zone ${source.hysteresis}`);
  if (source.debounce_ms > 0) parts.push(`debounce ${source.debounce_ms} ms`);
  if (source.device) parts.push(deviceLabel(source.device, config));
  return parts;
}

function bendRangeText(range: ValueRange): string {
  const signed = (value: number) => {
    const delta = value - 8192;
    return delta > 0 ? `+${delta}` : `${delta}`;
  };
  if (range.max >= 16383) return `≥ ${signed(range.min)}`;
  if (range.min <= 0) return `≤ ${signed(range.max)}`;
  return `${signed(range.min)}…${signed(range.max)}`;
}

export function deviceLabel(name: string, config: Config | null): string {
  const alias = config?.devices.find((device) => device.name === name)?.alias;
  return alias || name;
}

export function stepText(step: SequenceStep): string {
  switch (step.type) {
    case "keys":
      return step.keys.length ? comboText(step.keys) : "(no keys)";
    case "key_down":
      return `${keyLabel(step.key)} down`;
    case "key_up":
      return `${keyLabel(step.key)} up`;
    case "text":
      return `type “${truncate(step.text, 24)}”`;
    case "delay":
      return `wait ${formatMs(step.ms)}`;
    case "mouse_click":
      return clickText(step.button, step.clicks);
    case "mouse_down":
      return `${mouseButtonLabel(step.button).toLowerCase()} button down`;
    case "mouse_up":
      return `${mouseButtonLabel(step.button).toLowerCase()} button up`;
    case "mouse_scroll":
      return scrollText(step.dx, step.dy);
    case "mouse_move":
      return moveText(step.x, step.y, step.absolute);
  }
}

function truncate(text: string, max: number): string {
  const clean = text.replace(/\n/g, "⏎");
  return clean.length > max ? `${clean.slice(0, max - 1)}…` : clean;
}

function clickText(button: string, clicks: number): string {
  const name = mouseButtonLabel(button as never).toLowerCase();
  if (clicks === 2) return `double ${name} click`;
  if (clicks === 3) return `triple ${name} click`;
  return `${name} click`;
}

function scrollText(dx: number, dy: number): string {
  const parts: string[] = [];
  if (dy !== 0) parts.push(`${dy > 0 ? "up" : "down"} ${Math.abs(dy)}`);
  if (dx !== 0) parts.push(`${dx > 0 ? "right" : "left"} ${Math.abs(dx)}`);
  return parts.length ? `scroll ${parts.join(", ")}` : "scroll (nothing)";
}

function moveText(x: number, y: number, absolute: boolean): string {
  return absolute ? `move pointer to ${x}, ${y}` : `move pointer by ${x}, ${y}`;
}

/** One-line description of an action: "Ctrl+Shift+O", "type “hello”", "scroll up 3"… */
export function actionText(action: Action | null, config: Config | null = null): string {
  if (!action) return "nothing";
  switch (action.type) {
    case "keys":
      return action.keys.length ? comboText(action.keys) : "no keys yet";
    case "text":
      return action.text ? `type “${truncate(action.text, 28)}”` : "type (empty)";
    case "mouse_click":
      return clickText(action.button, action.clicks);
    case "mouse_scroll":
      return scrollText(action.dx, action.dy);
    case "mouse_move":
      return moveText(action.x, action.y, action.absolute);
    case "sequence":
      return action.steps.length ? `macro · ${plural(action.steps.length, "step")}` : "empty macro";
    case "switch_profile":
      if (action.target.type === "next") return "next profile";
      if (action.target.type === "previous") return "previous profile";
      if (action.target.type === "by_value") return "profile picked by the value";
      {
        const id = action.target.id;
        const profile = config?.profiles.find((p) => p.id === id);
        return `switch to ${profile?.name ?? "a missing profile"}`;
      }
    case "layer":
      return `${action.op === "toggle" ? "toggle" : `turn ${action.op}`} layer “${action.layer}”`;
    case "engine":
      return {
        pause: "pause output",
        resume: "resume output",
        toggle_pause: "pause / resume output",
        release_all: "release all keys",
      }[action.op];
    case "run":
      return action.command ? `run ${truncate(basename(action.command), 28)}` : "run (nothing)";
    case "open":
      return action.target ? `open ${truncate(action.target, 28)}` : "open (nothing)";
  }
}

function basename(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

export function targetText(target: HoldTarget): string {
  switch (target.type) {
    case "keys":
      return target.keys.length ? comboText(target.keys) : "no keys yet";
    case "mouse_button":
      return `${mouseButtonLabel(target.button).toLowerCase()} mouse button`;
    case "layer":
      return `layer “${target.layer}”`;
  }
}

export function rateLimitText(limit: RateLimit): string {
  const interval = formatMs(limit.interval_ms);
  if (limit.mode === "debounce") return `after ${interval} of quiet`;
  return limit.trailing ? `max once per ${interval} + final` : `max once per ${interval}`;
}

export function conditionText(condition: Condition, config: Config | null, octave = 4): string {
  switch (condition.type) {
    case "app": {
      const what = [condition.app.process, condition.app.title && `“${condition.app.title}”`]
        .filter(Boolean)
        .join(" ");
      return `${condition.negate ? "not in" : "in"} ${what || "any app"}`;
    }
    case "layer":
      return `while layer “${condition.layer}” is ${condition.active ? "on" : "off"}`;
    case "note_held":
      return `while ${noteName(condition.note, octave)} is ${condition.held ? "held" : "not held"}`;
    case "control_value":
      return `while CC ${condition.controller} ${rangeText(condition.range, 127).replace("= ", "is ")}`;
  }
  void config;
}

/** What a mapping does, in as few words as possible: its name, or "Ctrl+Shift+O", "hold Space"… */
export function mappingBrief(mapping: Mapping, config: Config | null = null): string {
  if (mapping.name) return mapping.name;
  const b = mapping.behavior;
  const several = (actions: (Action | null)[]) =>
    actions
      .filter((action): action is Action => action !== null)
      .map((action) => actionText(action, config))
      .join(" / ") || "nothing yet";
  switch (b.type) {
    case "trigger":
      return several([b.on_press, b.on_release]);
    case "hold":
      return `hold ${targetText(b.target)}`;
    case "toggle":
      return `toggle ${targetText(b.target)}`;
    case "cycle":
      return several(b.actions);
    case "tap_hold":
      return [b.tap && actionText(b.tap, config), b.hold && actionText(b.hold, config), b.hold_target && `hold ${targetText(b.hold_target)}`]
        .filter(Boolean)
        .join(" / ") || "nothing yet";
    case "multi_tap":
      return several([b.single, b.double, b.triple, b.hold]);
    case "change":
      return actionText(b.action, config);
    case "step":
      return [b.up && `↑ ${actionText(b.up, config)}`, b.down && `↓ ${actionText(b.down, config)}`]
        .filter(Boolean)
        .join("  ") || "nothing yet";
  }
}

/** A complete sentence describing a mapping, for tooltips and screen readers. */
export function mappingSentence(mapping: Mapping, config: Config | null, octave = 4): string {
  const source = [sourceTitle(mapping.source.event, octave), ...sourceQualifiers(mapping.source, config)].join(", ");
  const b = mapping.behavior;
  let what: string;
  switch (b.type) {
    case "trigger":
      what = [
        b.on_press && `on press: ${actionText(b.on_press, config)}`,
        b.on_release && `on release: ${actionText(b.on_release, config)}`,
        b.repeat && `repeating every ${formatMs(b.repeat.interval_ms)}`,
      ]
        .filter(Boolean)
        .join("; ") || "does nothing yet";
      break;
    case "hold":
      what = `hold ${targetText(b.target)}`;
      break;
    case "toggle":
      what = `toggle ${targetText(b.target)}`;
      break;
    case "cycle":
      what = `cycle through ${b.actions.map((a) => actionText(a, config)).join(" → ")}`;
      break;
    case "tap_hold":
      what = [
        b.tap && `tap: ${actionText(b.tap, config)}`,
        b.hold && `hold: ${actionText(b.hold, config)}`,
        b.hold_target && `while held: ${targetText(b.hold_target)}`,
      ]
        .filter(Boolean)
        .join("; ");
      break;
    case "multi_tap":
      what = [
        b.single && `single: ${actionText(b.single, config)}`,
        b.double && `double: ${actionText(b.double, config)}`,
        b.triple && `triple: ${actionText(b.triple, config)}`,
        b.hold && `long press: ${actionText(b.hold, config)}`,
      ]
        .filter(Boolean)
        .join("; ");
      break;
    case "change":
      what = `on change: ${actionText(b.action, config)}${b.min_change > 1 ? ` (ignoring jitter under ${b.min_change})` : ""}`;
      break;
    case "step":
      what = [
        b.up && `up: ${actionText(b.up, config)}`,
        b.down && `down: ${actionText(b.down, config)}`,
        b.fast?.up && `fast up: ${actionText(b.fast.up, config)}`,
        b.fast?.down && `fast down: ${actionText(b.fast.down, config)}`,
      ]
        .filter(Boolean)
        .join("; ");
      break;
  }
  const extras = [
    mapping.rate_limit && rateLimitText(mapping.rate_limit),
    ...mapping.conditions.map((c) => conditionText(c, config, octave)),
  ].filter(Boolean);
  return `${source} → ${what}${extras.length ? ` (${extras.join(", ")})` : ""}`;
}
