// In-browser stand-in for the native backend. It persists the config in localStorage and runs a
// deliberately tiny imitation of the engine (enough to light up the monitor and the mapping
// rows), so the interface can be worked on in a plain browser tab. It never sends real input.

import type { Config } from "@/bindings/Config";
import type { EngineEvent } from "@/bindings/EngineEvent";
import type { EngineStatus } from "@/bindings/EngineStatus";
import type { Mapping } from "@/bindings/Mapping";
import type { MidiMessage } from "@/bindings/MidiMessage";
import type { PortStatus } from "@/bindings/PortStatus";
import type { Profile } from "@/bindings/Profile";
import type { RuntimeEvent } from "@/bindings/RuntimeEvent";
import type { Snapshot } from "@/bindings/Snapshot";

import type { Backend } from "./api";
import { actionText, targetText } from "./describe";
import { download } from "./utils";

const STORAGE_KEY = "m2k.mock.config";

function sampleConfig(): Config {
  const keys = (...k: string[]) => ({ type: "keys" as const, keys: k as never[], tap_ms: null });
  return {
    version: 1,
    settings: {
      theme: "system",
      middle_c_octave: 4,
      start_minimized: false,
      close_to_tray: true,
      launch_at_login: false,
      suspend_while_focused: true,
      auto_switch_profiles: true,
      connect_new_devices: true,
      tap_ms: 25,
      key_stagger_ms: 0,
      repeat_delay_ms: 400,
      repeat_interval_ms: 33,
      injector: { windows_key_mode: "auto", linux_backend: "auto" },
    },
    devices: [{ name: "Launchkey Mini MK3", alias: "Launchkey", enabled: true }],
    selected_profile: "default",
    profiles: [
      {
        id: "default",
        name: "Default",
        always_on: false,
        apps: [],
        mappings: [
          {
            id: "m1",
            name: "",
            enabled: true,
            source: {
              device: null,
              channel: null, hysteresis: 0, debounce_ms: 0,
              event: { type: "note", notes: { min: 60, max: 60 }, velocity: { min: 1, max: 127 } },
            },
            behavior: { type: "trigger", on_press: keys("ArrowUp"), on_release: null, repeat: null },
            conditions: [],
            rate_limit: null,
          },
          {
            id: "m2",
            name: "",
            enabled: true,
            source: {
              device: null,
              channel: 2, hysteresis: 0, debounce_ms: 0,
              event: { type: "control_change", controller: 20, range: { min: 0, max: 127 }, encoding: "absolute" },
            },
            behavior: { type: "change", action: keys("ControlLeft", "ShiftLeft", "KeyO"), min_change: 0 },
            conditions: [],
            rate_limit: { mode: "throttle", interval_ms: 500, trailing: false },
          },
          {
            id: "m3",
            name: "Sustain = Space",
            enabled: true,
            source: {
              device: "Launchkey Mini MK3",
              channel: null, hysteresis: 0, debounce_ms: 0,
              event: { type: "control_change", controller: 64, range: { min: 64, max: 127 }, encoding: "absolute" },
            },
            behavior: { type: "hold", target: { type: "keys", keys: ["Space"] }, auto_repeat: true },
            conditions: [],
            rate_limit: null,
          },
          {
            id: "m4",
            name: "",
            enabled: true,
            source: {
              device: null,
              channel: null, hysteresis: 0, debounce_ms: 0,
              event: {
                type: "control_change",
                controller: 21,
                range: { min: 0, max: 127 },
                encoding: "relative_twos_complement",
              },
            },
            behavior: {
              type: "step",
              up: keys("ControlLeft", "Equal"),
              down: keys("ControlLeft", "Minus"),
              step: 1,
              max_burst: 8,
              invert: false,
              fast: null,
            },
            conditions: [],
            rate_limit: null,
          },
          {
            id: "m5",
            name: "",
            enabled: false,
            source: {
              device: null,
              channel: 10, hysteresis: 0, debounce_ms: 0,
              event: { type: "note", notes: { min: 36, max: 36 }, velocity: { min: 1, max: 127 } },
            },
            behavior: {
              type: "tap_hold",
              tap: keys("Escape"),
              hold: null,
              hold_target: { type: "keys", keys: ["ControlLeft"] },
              hold_ms: 350,
            },
            conditions: [{ type: "layer", layer: "shift", active: false }],
            rate_limit: null,
          },
        ],
      },
      { id: "p2", name: "Photoshop", always_on: false, apps: [{ process: "Photoshop.exe", title: "" }], mappings: [] },
      { id: "p3", name: "Global", always_on: true, apps: [], mappings: [] },
    ],
  };
}

/**
 * The real backend runs imported files through the config loader, which fills in every optional
 * field. This does just enough of that for the UI (which relies on complete objects).
 */
function normalizeProfiles(data: { profiles?: Profile[] }): Profile[] {
  return (data.profiles ?? []).map((profile) => ({
    ...profile,
    always_on: profile.always_on ?? false,
    apps: profile.apps ?? [],
    mappings: (profile.mappings ?? []).map((m) => ({
      ...m,
      name: m.name ?? "",
      enabled: m.enabled ?? true,
      conditions: m.conditions ?? [],
      rate_limit: m.rate_limit ?? null,
      source: {
        ...m.source,
        device: m.source.device ?? null,
        channel: m.source.channel ?? null,
        hysteresis: m.source.hysteresis ?? 0,
        debounce_ms: m.source.debounce_ms ?? 0,
      },
    })),
  }));
}

function loadConfig(): Config {
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored) return JSON.parse(stored) as Config;
  } catch {
    // fall through to the sample
  }
  return sampleConfig();
}

function parse(bytes: number[]): MidiMessage {
  const [status = 0, a = 0, b = 0] = bytes;
  const channel = (status & 0x0f) + 1;
  switch (status & 0xf0) {
    case 0x80:
      return { type: "note_off", channel, note: a, velocity: b };
    case 0x90:
      return { type: "note_on", channel, note: a, velocity: b };
    case 0xa0:
      return { type: "poly_pressure", channel, note: a, pressure: b };
    case 0xb0:
      return { type: "control_change", channel, controller: a, value: b };
    case 0xc0:
      return { type: "program_change", channel, program: a };
    case 0xd0:
      return { type: "channel_pressure", channel, pressure: a };
    case 0xe0:
      return { type: "pitch_bend", channel, value: a | (b << 7) };
  }
  switch (status) {
    case 0xf0:
      return { type: "sys_ex", data: bytes };
    case 0xf8:
      return { type: "clock" };
    case 0xfa:
      return { type: "start" };
    case 0xfb:
      return { type: "continue" };
    case 0xfc:
      return { type: "stop" };
    default:
      return { type: "unknown", data: bytes };
  }
}

/** Splits a byte stream into messages at the status bytes (SysEx runs to its F7). */
function split(bytes: number[]): number[][] {
  const messages: number[][] = [];
  let start = 0;
  while (start < bytes.length) {
    let end = start + 1;
    if (bytes[start] === 0xf0) {
      const stop = bytes.indexOf(0xf7, start);
      end = stop < 0 ? bytes.length : stop + 1;
    } else {
      while (end < bytes.length && bytes[end] < 0x80) end++;
    }
    messages.push(bytes.slice(start, end));
    start = end;
  }
  return messages;
}

type Edge = "press" | "release" | "value";

function match(mapping: Mapping, device: string, message: MidiMessage): Edge | null {
  const { source } = mapping;
  if (source.device && source.device !== device) return null;
  if (source.channel !== null && "channel" in message && message.channel !== source.channel) return null;
  const event = source.event;
  if (event.type === "note" && (message.type === "note_on" || message.type === "note_off")) {
    if (message.note < event.notes.min || message.note > event.notes.max) return null;
    return message.type === "note_on" && message.velocity > 0 ? "press" : "release";
  }
  if (event.type === "control_change" && message.type === "control_change") {
    if (message.controller !== event.controller) return null;
    if (event.encoding !== "absolute") return "value";
    const inside = message.value >= event.range.min && message.value <= event.range.max;
    const full = event.range.min === 0 && event.range.max === 127;
    return full ? "value" : inside ? "press" : "release";
  }
  if (event.type === "program_change" && message.type === "program_change") {
    return message.program >= event.programs.min && message.program <= event.programs.max ? "press" : null;
  }
  if (event.type === "pitch_bend" && message.type === "pitch_bend") return "value";
  return null;
}

export function createMockBackend(): Backend {
  let config = loadConfig();
  const listeners = new Set<(event: RuntimeEvent) => void>();
  const started = performance.now();
  const lastFired = new Map<string, number>();
  const status: EngineStatus = {
    paused: false,
    focus_suspended: false,
    learning: false,
    selected_profile: config.selected_profile,
    effective_profile: config.selected_profile,
    auto_switched: false,
    layers: [],
    held_keys: [],
    engaged_mappings: [],
    foreground: null,
    backend: "mock",
    latency: null,
  };
  const ports: PortStatus[] = [
    { name: "Launchkey Mini MK3", state: "connected", error: "", is_virtual: false },
    { name: "Arturia BeatStep", state: "connected", error: "", is_virtual: false },
    { name: "loopMIDI Port", state: "error", error: "In use by another application", is_virtual: false },
  ];

  const emit = (events: EngineEvent[]) => {
    const event: RuntimeEvent = { type: "engine", events };
    listeners.forEach((listener) => listener(event));
  };
  const emitStatus = () => emit([{ type: "status", ...status }]);
  const now = () => Math.round((performance.now() - started) * 1000);

  const handle = (device: string, message: MidiMessage) => {
    const profile = config.profiles.find((p) => p.id === status.effective_profile);
    const live = config.profiles.filter((p) => p.always_on || p === profile);
    const matched: string[] = [];
    const fired: EngineEvent[] = [];
    for (const p of live) {
      for (const mapping of p.mappings) {
        if (!mapping.enabled) continue;
        const edge = match(mapping, device, message);
        if (!edge) continue;
        matched.push(mapping.id);
        const b = mapping.behavior;
        let description: string | null = null;
        if (b.type === "trigger" && edge !== "release" && b.on_press) description = actionText(b.on_press, config);
        else if (b.type === "trigger" && edge === "release" && b.on_release) description = actionText(b.on_release, config);
        else if (b.type === "change" && edge !== "release") description = actionText(b.action, config);
        else if (b.type === "hold") description = `${edge === "release" ? "release" : "hold"} ${targetText(b.target)}`;
        else if (b.type === "toggle" && edge === "press") description = `toggle ${targetText(b.target)}`;
        else if (b.type === "step" && edge === "value" && (b.up || b.down)) description = actionText(b.up ?? b.down, config);
        if (!description) continue;
        const limit = mapping.rate_limit;
        const last = lastFired.get(mapping.id) ?? -Infinity;
        const limited = limit !== null && performance.now() - last < limit.interval_ms;
        if (!limited) lastFired.set(mapping.id, performance.now());
        fired.push({
          type: "fired",
          time_us: now(),
          profile_id: p.id,
          mapping_id: mapping.id,
          description,
          outcome: status.learning
            ? { type: "skipped", reason: "learning", detail: "" }
            : status.paused
              ? { type: "skipped", reason: "paused", detail: "" }
              : limited
                ? { type: "skipped", reason: "rate_limited", detail: "" }
                : { type: "sent" },
        });
      }
    }
    emit([{ type: "midi", time_us: now(), device, message, derived: [], mappings: matched }, ...fired]);
  };

  // Lets scripts and the browser console play MIDI into the mock: __m2k.send("Dev", [0x90, 60, 100]).
  (window as unknown as Record<string, unknown>).__m2k = {
    send: (device: string, bytes: number[]) => split(bytes).forEach((message) => handle(device, parse(message))),
  };

  return {
    async snapshot(): Promise<Snapshot> {
      return {
        info: { version: "0.1.0-mock", platform: "windows", config_path: "(browser storage)", wayland: false, elevated: false },
        config,
        engine: { ...status },
        ports,
        permissions: { can_inject: true, can_detect_foreground: true, hint: "" },
        warnings: [],
        backend_error: "",
        unsupported_keys: ["BrightnessUp", "BrightnessDown", "Eject", "Power", "WakeUp", "NumpadEqual"],
      };
    },
    async saveConfig(next) {
      config = next;
      localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
      if (status.selected_profile !== next.selected_profile) {
        status.selected_profile = next.selected_profile;
        status.effective_profile = next.selected_profile;
        emitStatus();
      }
      return next;
    },
    async setPaused(paused) {
      status.paused = paused;
      emitStatus();
    },
    async setLearning(learning) {
      status.learning = learning;
      emitStatus();
    },
    async sendMidi(device, bytes) {
      split(bytes).forEach((message) => handle(device, parse(message)));
    },
    async testAction(action, delayMs) {
      setTimeout(() => {
        emit([
          {
            type: "fired",
            time_us: now(),
            profile_id: "",
            mapping_id: "",
            description: actionText(action, config),
            outcome: { type: "sent" },
          },
        ]);
      }, delayMs);
    },
    async releaseAll() {},
    async rescanDevices() {},
    async requestPermissions() {},
    async runningApps() {
      return [
        { process: "Ableton Live 12 Suite.exe", title: "Untitled - Ableton Live 12 Suite", app_id: "", pid: 1, elevated: false },
        { process: "chrome.exe", title: "New Tab - Google Chrome", app_id: "", pid: 2, elevated: false },
        { process: "Photoshop.exe", title: "Untitled-1 @ 100%", app_id: "", pid: 3, elevated: false },
        { process: "obs64.exe", title: "OBS Studio", app_id: "", pid: 4, elevated: false },
      ];
    },
    async revealConfig() {},
    async exportProfiles(profiles: Profile[], suggestedName: string) {
      download(suggestedName, JSON.stringify({ format: "midi-2-keyboard-profiles", version: 1, profiles }, null, 2));
      return true;
    },
    async importProfiles() {
      return new Promise<Profile[] | null>((resolve) => {
        const input = document.createElement("input");
        input.type = "file";
        input.accept = ".json,application/json";
        input.onchange = async () => {
          const file = input.files?.[0];
          if (!file) return resolve(null);
          try {
            const data = JSON.parse(await file.text());
            resolve(Array.isArray(data.profiles) ? normalizeProfiles(data) : null);
          } catch {
            resolve(null);
          }
        };
        input.oncancel = () => resolve(null);
        input.click();
      });
    },
    async parseProfiles(text: string) {
      return normalizeProfiles(JSON.parse(text));
    },
    async restartAsAdmin() {},
    async quit() {},
    async subscribe(listener) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
  };
}
