// The one seam between the UI and the native side. `backend` is the Tauri implementation inside
// the app and an in-browser mock everywhere else (`npm run dev` in a normal browser), so the whole
// interface can be developed and reviewed without the desktop shell.

import type { Action } from "@/bindings/Action";
import type { Config } from "@/bindings/Config";
import type { ForegroundApp } from "@/bindings/ForegroundApp";
import type { Profile } from "@/bindings/Profile";
import type { RuntimeEvent } from "@/bindings/RuntimeEvent";
import type { Snapshot } from "@/bindings/Snapshot";

import { createMockBackend } from "./api.mock";
import { isTauri } from "./platform";

export interface Backend {
  /** Everything needed for the first render. */
  snapshot(): Promise<Snapshot>;
  /** Persists and applies a config. Resolves with the config as the backend sanitized it. */
  saveConfig(config: Config): Promise<Config>;
  setPaused(paused: boolean): Promise<void>;
  /** While learning, mappings are matched and reported but nothing is sent to the OS. */
  setLearning(learning: boolean): Promise<void>;
  /** Feeds MIDI bytes into the engine as if `device` had sent them. */
  sendMidi(device: string, bytes: number[]): Promise<void>;
  /** Runs an action after `delayMs`, ignoring pause / focus suspension. */
  testAction(action: Action, delayMs: number): Promise<void>;
  releaseAll(): Promise<void>;
  rescanDevices(): Promise<void>;
  requestPermissions(): Promise<void>;
  /** Applications that currently have a visible window (for "pick an app" lists). */
  runningApps(): Promise<ForegroundApp[]>;
  /** Shows the config file in the system file manager. */
  revealConfig(): Promise<void>;
  /** Saves profiles to a file chosen by the user. Resolves `false` if cancelled. */
  exportProfiles(profiles: Profile[], suggestedName: string): Promise<boolean>;
  /** Lets the user pick a file and returns the profiles inside it (`null` if cancelled). */
  importProfiles(): Promise<Profile[] | null>;
  quit(): Promise<void>;
  subscribe(listener: (event: RuntimeEvent) => void): Promise<() => void>;
}

async function createTauriBackend(): Promise<Backend> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  return {
    snapshot: () => invoke<Snapshot>("snapshot"),
    saveConfig: (config) => invoke<Config>("save_config", { config }),
    setPaused: (paused) => invoke("set_paused", { paused }),
    setLearning: (learning) => invoke("set_learning", { learning }),
    sendMidi: (device, bytes) => invoke("send_midi", { device, bytes }),
    testAction: (action, delayMs) => invoke("test_action", { action, delayMs }),
    releaseAll: () => invoke("release_all"),
    rescanDevices: () => invoke("rescan_devices"),
    requestPermissions: () => invoke("request_permissions"),
    runningApps: () => invoke<ForegroundApp[]>("running_apps"),
    revealConfig: () => invoke("reveal_config"),
    exportProfiles: (profiles, suggestedName) =>
      invoke<boolean>("export_profiles", { profiles, suggestedName }),
    importProfiles: () => invoke<Profile[] | null>("import_profiles"),
    quit: () => invoke("quit"),
    subscribe: async (listener) => {
      const unlisten = await listen<RuntimeEvent>("m2k:event", (event) => listener(event.payload));
      return unlisten;
    },
  };
}

let instance: Promise<Backend> | null = null;

export function getBackend(): Promise<Backend> {
  instance ??= isTauri ? createTauriBackend() : Promise.resolve(createMockBackend());
  return instance;
}
