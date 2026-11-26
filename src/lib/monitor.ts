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

// Under a flood of MIDI the engine reports sixty batches a second. Nobody can read or see that
// fast, and redrawing for every batch is what costs CPU: the log is redrawn at most twenty
// times a second, and a light that was just lit is not lit again for a moment (it stays on
// either way) unless something more important happened to its mapping.
const LOG_REDRAW_MS = 50;
const PULSE_REPEAT_MS = 90;
let logRedrawTimer: ReturnType<typeof setTimeout> | null = null;
let lastLogRedraw = 0;

function notifyLogSoon() {
  const wait = LOG_REDRAW_MS - (performance.now() - lastLogRedraw);
  if (wait <= 0 && logRedrawTimer === null) {
    lastLogRedraw = performance.now();
    notifyLog();
  } else if (logRedrawTimer === null) {
    logRedrawTimer = setTimeout(() => {
      logRedrawTimer = null;
      lastLogRedraw = performance.now();
      notifyLog();
    }, Math.max(0, wait));
  }
}

const PULSE_RANK: Record<ActivityKind, number> = { matched: 0, skipped: 1, sent: 2, failed: 3 };
const lastPulse = new Map<string, { at: number; rank: number }>();
const lastDevicePulse = new Map<string, number>();

function pulse(mappingId: string, kind: ActivityKind) {
  const listeners = activityListeners.get(mappingId);
  if (!listeners) return;
  const now = performance.now();
  const rank = PULSE_RANK[kind];
  const last = lastPulse.get(mappingId);
  if (last && now - last.at < PULSE_REPEAT_MS && rank <= last.rank) return;
  lastPulse.set(mappingId, { at: now, rank });
  listeners.forEach((listener) => listener(kind));
}

function devicePulse(device: string) {
  const listeners = deviceListeners.get(device);
  if (!listeners) return;
  const now = performance.now();
  if (now - (lastDevicePulse.get(device) ?? -Infinity) < PULSE_REPEAT_MS) return;
  lastDevicePulse.set(device, now);
  listeners.forEach((listener) => listener());
}

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
  devices.forEach(devicePulse);
  if (added.length === 0 || frozen) return;
  entries = entries.concat(added);
  if (entries.length > MAX_ENTRIES) entries = entries.slice(entries.length - MAX_ENTRIES);
  notifyLogSoon();
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
    if (set.size === 0) {
      activityListeners.delete(mappingId);
      lastPulse.delete(mappingId);
    }
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
