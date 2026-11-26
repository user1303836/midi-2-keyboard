// High-frequency side of the app state: the live event log and per-mapping activity pulses.
// Kept out of the main (zustand) store so a busy MIDI stream never re-renders the editor; the
// components that care subscribe here directly.

import { useSyncExternalStore } from "react";

import type { EngineEvent } from "@/bindings/EngineEvent";
import type { FiredLog } from "@/bindings/FiredLog";
import type { MidiLog } from "@/bindings/MidiLog";

export type LogEntry =
  | ({ kind: "midi"; id: number } & MidiLog)
  | ({ kind: "fired"; id: number } & FiredLog)
  | { kind: "error"; id: number; time_us: number; message: string };

const MAX_ENTRIES = 600;

let entries: LogEntry[] = [];
let nextId = 1;
let frozen = false;
let lastMidi: MidiLog | null = null;
const logListeners = new Set<() => void>();
const midiListeners = new Set<(log: MidiLog) => void>();
const firedListeners = new Set<(log: FiredLog) => void>();

export type ActivityKind = "matched" | "sent" | "skipped" | "failed";
const activityListeners = new Map<string, Set<(kind: ActivityKind) => void>>();
const deviceListeners = new Map<string, Set<() => void>>();

function notifyLog() {
  logListeners.forEach((listener) => listener());
}

function pulse(mappingId: string, kind: ActivityKind) {
  activityListeners.get(mappingId)?.forEach((listener) => listener(kind));
}

const PULSE_RANK: Record<ActivityKind, number> = { matched: 0, skipped: 1, sent: 2, failed: 3 };

/** Feeds a batch of engine events (status events are handled by the main store). */
export function pushEngineEvents(events: EngineEvent[]): void {
  const added: LogEntry[] = [];
  let lastTime = 0;
  // A busy controller can touch the same mapping hundreds of times in one batch; every light
  // flashes at most once per batch, showing the most significant thing that happened.
  const pulses = new Map<string, ActivityKind>();
  const devices = new Set<string>();
  const notePulse = (id: string, kind: ActivityKind) => {
    const current = pulses.get(id);
    if (current === undefined || PULSE_RANK[kind] > PULSE_RANK[current]) pulses.set(id, kind);
  };
  for (const event of events) {
    switch (event.type) {
      case "midi": {
        const { type: _type, ...log } = event;
        lastMidi = log;
        lastTime = log.time_us;
        midiListeners.forEach((listener) => listener(log));
        devices.add(log.device);
        for (const id of log.mappings) notePulse(id, "matched");
        added.push({ kind: "midi", id: nextId++, ...log });
        break;
      }
      case "fired": {
        const { type: _type, ...log } = event;
        lastTime = log.time_us;
        firedListeners.forEach((listener) => listener(log));
        notePulse(
          log.mapping_id,
          log.outcome.type === "sent" ? "sent" : log.outcome.type === "failed" ? "failed" : "skipped",
        );
        added.push({ kind: "fired", id: nextId++, ...log });
        break;
      }
      case "error":
        added.push({ kind: "error", id: nextId++, time_us: lastTime, message: event.message });
        break;
      case "status":
        break;
    }
  }
  pulses.forEach((kind, id) => pulse(id, kind));
  devices.forEach((device) => deviceListeners.get(device)?.forEach((listener) => listener()));
  if (added.length === 0 || frozen) return;
  entries = entries.concat(added);
  if (entries.length > MAX_ENTRIES) entries = entries.slice(entries.length - MAX_ENTRIES);
  notifyLog();
}

export function clearLog(): void {
  entries = [];
  notifyLog();
}

export function setLogFrozen(value: boolean): void {
  frozen = value;
  notifyLog();
}

export function isLogFrozen(): boolean {
  return frozen;
}

function subscribeLog(listener: () => void): () => void {
  logListeners.add(listener);
  return () => logListeners.delete(listener);
}

/** The live log (newest last). */
export function useLog(): LogEntry[] {
  return useSyncExternalStore(subscribeLog, () => entries);
}

export function useLogFrozen(): boolean {
  return useSyncExternalStore(subscribeLog, () => frozen);
}

/** Every incoming MIDI message, as it arrives (MIDI learn, live value displays). */
export function onMidi(listener: (log: MidiLog) => void): () => void {
  midiListeners.add(listener);
  return () => midiListeners.delete(listener);
}

export function onFired(listener: (log: FiredLog) => void): () => void {
  firedListeners.add(listener);
  return () => firedListeners.delete(listener);
}

export function getLastMidi(): MidiLog | null {
  return lastMidi;
}

/** Activity pulses of one mapping (its source matched / its action was sent or skipped). */
export function onMappingActivity(mappingId: string, listener: (kind: ActivityKind) => void): () => void {
  let set = activityListeners.get(mappingId);
  if (!set) {
    set = new Set();
    activityListeners.set(mappingId, set);
  }
  set.add(listener);
  return () => {
    set.delete(listener);
    if (set.size === 0) activityListeners.delete(mappingId);
  };
}

/** Fires whenever `device` sends anything (the activity LED next to a device). */
export function onDeviceActivity(device: string, listener: () => void): () => void {
  let set = deviceListeners.get(device);
  if (!set) {
    set = new Set();
    deviceListeners.set(device, set);
  }
  set.add(listener);
  return () => {
    set.delete(listener);
    if (set.size === 0) deviceListeners.delete(device);
  };
}
