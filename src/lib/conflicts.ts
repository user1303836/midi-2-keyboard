// Spots mappings that react to the same control under the same conditions. That is allowed
// (both fire) and sometimes intended, but far more often it is an accident — so the list points
// it out instead of leaving the user to wonder why two things happen.

import type { Config } from "@/bindings/Config";
import type { Mapping } from "@/bindings/Mapping";
import type { MidiMessage } from "@/bindings/MidiMessage";
import type { Profile } from "@/bindings/Profile";
import type { Source } from "@/bindings/Source";
import type { SourceEvent } from "@/bindings/SourceEvent";
import type { ValueRange } from "@/bindings/ValueRange";

import { deepEqual } from "./utils";

function overlap(a: ValueRange, b: ValueRange): boolean {
  return a.min <= b.max && b.min <= a.max;
}

function within(range: ValueRange, value: number): boolean {
  return value >= range.min && value <= range.max;
}

/** Does `source` belong to the control this message came from, whatever value it sent? */
export function listensTo(source: Source, device: string, message: MidiMessage): boolean {
  if (source.device !== null && source.device !== device) return false;
  if (source.channel !== null && "channel" in message && message.channel !== source.channel) return false;
  const event = source.event;
  switch (message.type) {
    case "note_on":
    case "note_off":
      return (
        (event.type === "note" && within(event.notes, message.note)) ||
        (event.type === "chord" && event.notes.includes(message.note))
      );
    case "control_change":
      return (
        (event.type === "control_change" && event.controller === message.controller) ||
        (event.type === "control_change14" &&
          (event.controller === message.controller || event.controller + 32 === message.controller))
      );
    case "program_change":
      return event.type === "program_change" && within(event.programs, message.program);
    case "pitch_bend":
      return event.type === "pitch_bend";
    case "channel_pressure":
      return event.type === "channel_pressure";
    case "poly_pressure":
      return event.type === "poly_pressure" && within(event.notes, message.note);
    case "song_select":
      return event.type === "song_select" && within(event.songs, message.song);
    case "song_position":
      return event.type === "song_position";
    default:
      return event.type === "system" && (event.message as string) === message.type;
  }
}

/**
 * Why a message that no live mapping reacted to did nothing, when the configuration can tell:
 * its mapping is switched off, the value is outside what its mapping reacts to, or it is only
 * mapped in a profile that is not live right now. `null` = nothing listens to it anywhere;
 * `""` = a live mapping does listen to this control (the message is simply older than the
 * mapping, or not one it acts on), so there is nothing to explain.
 */
export function silentReason(
  config: Config,
  liveProfileId: string | null,
  device: string,
  message: MidiMessage,
): string | null {
  let outside = false;
  let listening = false;
  let elsewhere: string | null = null;
  for (const profile of config.profiles) {
    const live = profile.always_on || profile.id === liveProfileId;
    for (const mapping of profile.mappings) {
      if (!listensTo(mapping.source, device, message)) continue;
      if (!live) {
        if (mapping.enabled) elsewhere ??= profile.name;
        continue;
      }
      if (!mapping.enabled) return "its mapping is switched off";
      listening = true;
      const event = mapping.source.event;
      if (event.type === "note" && message.type === "note_on" && message.velocity > 0) {
        outside ||= !within(event.velocity, message.velocity);
      } else if (event.type === "control_change" && event.encoding === "absolute" && message.type === "control_change") {
        outside ||= !within(event.range, message.value);
      }
    }
  }
  if (outside) return "outside the range its mapping reacts to";
  if (listening) return "";
  return elsewhere ? `only mapped in “${elsewhere}”` : null;
}

function sameControl(a: SourceEvent, b: SourceEvent): boolean {
  if (a.type !== b.type) return false;
  switch (a.type) {
    case "note": {
      const other = b as typeof a;
      return overlap(a.notes, other.notes) && overlap(a.velocity, other.velocity);
    }
    case "chord": {
      const other = b as typeof a;
      return a.notes.length === other.notes.length && a.notes.every((note) => other.notes.includes(note));
    }
    case "control_change": {
      const other = b as typeof a;
      if (a.controller !== other.controller) return false;
      // Relative encoders ignore the range; absolute ones only clash where their ranges meet.
      return a.encoding !== "absolute" || other.encoding !== "absolute" || overlap(a.range, other.range);
    }
    case "control_change14": {
      const other = b as typeof a;
      return a.controller === other.controller && overlap(a.range, other.range);
    }
    case "nrpn":
    case "rpn": {
      const other = b as typeof a;
      return a.number === other.number && overlap(a.range, other.range);
    }
    case "program_change":
      return overlap(a.programs, (b as typeof a).programs);
    case "pitch_bend":
    case "channel_pressure":
    case "song_position":
      return overlap(a.range, (b as typeof a).range);
    case "poly_pressure": {
      const other = b as typeof a;
      return overlap(a.notes, other.notes) && overlap(a.range, other.range);
    }
    case "sys_ex":
      return a.pattern.trim().toUpperCase() === (b as typeof a).pattern.trim().toUpperCase();
    case "song_select":
      return overlap(a.songs, (b as typeof a).songs);
    case "system":
      return a.message === (b as typeof a).message;
    case "clock":
      return a.divisor === (b as typeof a).divisor;
  }
}

function clashes(a: Mapping, b: Mapping): boolean {
  if (!a.enabled || !b.enabled) return false;
  const sa = a.source;
  const sb = b.source;
  if (sa.device !== null && sb.device !== null && sa.device !== sb.device) return false;
  if (sa.channel !== null && sb.channel !== null && sa.channel !== sb.channel) return false;
  if (!sameControl(sa.event, sb.event)) return false;
  // Different conditions are the normal way to give one control several jobs.
  return deepEqual(a.conditions, b.conditions);
}

export interface Conflict {
  mapping: Mapping;
  /** Name of the profile the other mapping lives in, when it is not the viewed one. */
  profile: string | null;
}

/** For every mapping of `profile`: the other live mappings that react to the same control. */
export function findConflicts(config: Config, profile: Profile): Map<string, Conflict[]> {
  const result = new Map<string, Conflict[]>();
  const others: { mapping: Mapping; profile: string | null }[] = profile.mappings.map((mapping) => ({
    mapping,
    profile: null,
  }));
  for (const other of config.profiles) {
    // Always-on profiles are live together with every profile.
    if (other.id === profile.id || !(other.always_on || profile.always_on)) continue;
    if (profile.always_on && !other.always_on) continue; // would flag every profile; too noisy
    for (const mapping of other.mappings) others.push({ mapping, profile: other.name });
  }
  for (const mapping of profile.mappings) {
    const found = others.filter((o) => o.mapping.id !== mapping.id && clashes(mapping, o.mapping));
    if (found.length > 0) result.set(mapping.id, found);
  }
  return result;
}
