// Spots mappings that react to the same control under the same conditions. That is allowed
// (both fire) and sometimes intended, but far more often it is an accident — so the list points
// it out instead of leaving the user to wonder why two things happen.

import type { Config } from "@/bindings/Config";
import type { Mapping } from "@/bindings/Mapping";
import type { Profile } from "@/bindings/Profile";
import type { SourceEvent } from "@/bindings/SourceEvent";
import type { ValueRange } from "@/bindings/ValueRange";

import { deepEqual } from "./utils";

function overlap(a: ValueRange, b: ValueRange): boolean {
  return a.min <= b.max && b.min <= a.max;
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
