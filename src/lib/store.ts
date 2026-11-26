// The main application store: the config being edited (with undo/redo and debounced saving),
// the engine's live status, and UI state. High-frequency data lives in `monitor.ts`.

import { useMemo } from "react";
import { create } from "zustand";

import type { AppInfo } from "@/bindings/AppInfo";
import type { Config } from "@/bindings/Config";
import type { DeviceConfig } from "@/bindings/DeviceConfig";
import type { EngineStatus } from "@/bindings/EngineStatus";
import type { Mapping } from "@/bindings/Mapping";
import type { Permissions } from "@/bindings/Permissions";
import type { PortStatus } from "@/bindings/PortStatus";
import type { Profile } from "@/bindings/Profile";
import type { RuntimeEvent } from "@/bindings/RuntimeEvent";
import type { Settings } from "@/bindings/Settings";

import { getBackend, type Backend } from "./api";
import { loadKeyboardLayout } from "./keys";
import { duplicateMapping, duplicateProfile, knownLayers, newMapping, newProfile } from "./model";
import { pushEngineEvents } from "./monitor";
import { setPlatform } from "./platform";
import { deepEqual, move } from "./utils";

export interface Toast {
  id: number;
  kind: "info" | "success" | "warning" | "error";
  message: string;
  action?: { label: string; run: () => void };
}

export type Panel = "none" | "settings" | "devices";

interface State {
  ready: boolean;
  fatal: string | null;
  info: AppInfo;
  config: Config;
  engine: EngineStatus;
  ports: PortStatus[];
  permissions: Permissions;
  backendError: string;

  past: Config[];
  future: Config[];
  saving: boolean;

  /** The profile shown in the main view (may be an always-on profile that is not "selected"). */
  viewProfileId: string | null;
  /** The mapping open in the editor. */
  selectedMappingId: string | null;
  /** The mapping currently going through the quick-add flow. */
  wizardMappingId: string | null;
  /** A quick-add mapping that is not part of the config yet (nothing learned so far). */
  draft: Mapping | null;
  panel: Panel;
  monitorOpen: boolean;
  monitorTab: "log" | "virtual";
  search: string;
  toasts: Toast[];

  init(): Promise<void>;
  edit(change: (config: Config) => Config, coalesceKey?: string): void;
  undo(): void;
  redo(): void;

  viewProfile(id: string): void;
  addProfile(name: string): string;
  updateProfile(id: string, change: (profile: Profile) => Profile, coalesceKey?: string): void;
  removeProfile(id: string): void;
  cloneProfile(id: string): void;
  moveProfile(id: string, toIndex: number): void;
  importProfiles(profiles: Profile[]): void;

  addMapping(mapping: Mapping, options?: { wizard?: boolean; index?: number }): void;
  updateMapping(id: string, change: (mapping: Mapping) => Mapping, coalesceKey?: string): void;
  removeMapping(id: string): void;
  cloneMapping(id: string): void;
  moveMapping(id: string, toIndex: number): void;
  moveMappingToProfile(id: string, profileId: string): void;
  selectMapping(id: string | null): void;
  /** Opens the quick-add flow with a fresh draft mapping. */
  startWizard(): void;
  /** Moves the draft into the viewed profile (it becomes a real, saved mapping). */
  commitDraft(): void;
  finishWizard(): void;

  updateSettings(change: (settings: Settings) => Settings, coalesceKey?: string): void;
  updateDevice(name: string, change: (device: DeviceConfig) => DeviceConfig, coalesceKey?: string): void;
  forgetDevice(name: string): void;

  setPaused(paused: boolean): void;
  setPanel(panel: Panel): void;
  setMonitorOpen(open: boolean): void;
  setMonitorTab(tab: "log" | "virtual"): void;
  /** Opens the monitor on the on-screen controller. */
  openVirtualController(): void;
  setSearch(search: string): void;
  toast(message: string, options?: Partial<Omit<Toast, "id" | "message">>): void;
  dismissToast(id: number): void;
}

const EMPTY_STATUS: EngineStatus = {
  paused: false,
  focus_suspended: false,
  learning: false,
  selected_profile: null,
  effective_profile: null,
  auto_switched: false,
  layers: [],
  held_keys: [],
  engaged_mappings: [],
  foreground: null,
  backend: "",
};

const EMPTY_CONFIG = { version: 1, settings: {}, devices: [], profiles: [], selected_profile: null } as unknown as Config;

const HISTORY_LIMIT = 100;
const COALESCE_MS = 900;
const SAVE_DELAY_MS = 220;

let backend: Backend | null = null;
let saveTimer: ReturnType<typeof setTimeout> | null = null;
let lastEditKey: string | null = null;
let lastEditAt = 0;
let toastId = 1;
let initStarted = false;

function mapProfile(config: Config, id: string, change: (profile: Profile) => Profile): Config {
  let touched = false;
  const profiles = config.profiles.map((profile) => {
    if (profile.id !== id) return profile;
    const next = change(profile);
    if (next !== profile) touched = true;
    return next;
  });
  return touched ? { ...config, profiles } : config;
}

function mapMapping(config: Config, id: string, change: (mapping: Mapping) => Mapping): Config {
  let touched = false;
  const profiles = config.profiles.map((profile) => {
    const index = profile.mappings.findIndex((mapping) => mapping.id === id);
    if (index < 0) return profile;
    const next = change(profile.mappings[index]);
    if (next === profile.mappings[index]) return profile;
    touched = true;
    const mappings = profile.mappings.slice();
    mappings[index] = next;
    return { ...profile, mappings };
  });
  return touched ? { ...config, profiles } : config;
}

export function profileOfMapping(config: Config, mappingId: string): Profile | null {
  return config.profiles.find((p) => p.mappings.some((m) => m.id === mappingId)) ?? null;
}

export const useApp = create<State>((set, get) => {
  const scheduleSave = () => {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = setTimeout(flushSave, SAVE_DELAY_MS);
  };

  const flushSave = async () => {
    if (saveTimer) clearTimeout(saveTimer);
    saveTimer = null;
    if (!backend) return;
    const sent = get().config;
    set({ saving: true });
    try {
      const saved = await backend.saveConfig(sent);
      // Adopt the backend's sanitized version unless the user has edited again in the meantime.
      if (get().config === sent && !deepEqual(saved, sent)) set({ config: saved });
    } catch (error) {
      get().toast(`Could not save: ${String(error)}`, { kind: "error" });
    } finally {
      set({ saving: false });
    }
  };

  const onRuntimeEvent = (event: RuntimeEvent) => {
    switch (event.type) {
      case "engine": {
        pushEngineEvents(event.events);
        for (const e of event.events) {
          if (e.type !== "status") continue;
          const { type: _type, ...engine } = e;
          const state = get();
          const patch: Partial<State> = { engine };
          // A "switch profile" action (or another window) changed the selection: follow it.
          if (engine.selected_profile && engine.selected_profile !== state.config.selected_profile) {
            patch.config = { ...state.config, selected_profile: engine.selected_profile };
            const viewed = state.config.profiles.find((p) => p.id === state.viewProfileId);
            if (!viewed?.always_on) {
              patch.viewProfileId = engine.selected_profile;
              patch.selectedMappingId = null;
            }
          }
          set(patch);
        }
        break;
      }
      case "ports":
        set({ ports: event.ports });
        break;
      case "permissions":
        set({ permissions: event.permissions, backendError: event.backend_error });
        break;
      case "config":
        set({ config: event.config, past: [], future: [] });
        break;
    }
  };

  // Flush pending edits when the window is hidden or closed.
  if (typeof window !== "undefined") {
    // Only when an edit is actually waiting: an unconditional save here would write this
    // window's copy over changes made elsewhere (the tray, another window, an import).
    window.addEventListener("beforeunload", () => {
      if (saveTimer) void flushSave();
    });
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "hidden" && saveTimer) void flushSave();
    });
  }

  return {
    ready: false,
    fatal: null,
    info: { version: "", platform: "", config_path: "", wayland: false },
    config: EMPTY_CONFIG,
    engine: EMPTY_STATUS,
    ports: [],
    permissions: { can_inject: true, can_detect_foreground: true, hint: "" },
    backendError: "",
    past: [],
    future: [],
    saving: false,
    viewProfileId: null,
    selectedMappingId: null,
    wizardMappingId: null,
    draft: null,
    panel: "none",
    monitorOpen: false,
    monitorTab: "log",
    search: "",
    toasts: [],

    async init() {
      // React's strict mode runs effects twice in development; start only once.
      if (initStarted) return;
      initStarted = true;
      try {
        backend = await getBackend();
        await backend.subscribe(onRuntimeEvent);
        const [snapshot] = await Promise.all([backend.snapshot(), loadKeyboardLayout()]);
        setPlatform(snapshot.info.platform);
        set({
          ready: true,
          info: snapshot.info,
          config: snapshot.config,
          engine: snapshot.engine,
          ports: snapshot.ports,
          permissions: snapshot.permissions,
          backendError: snapshot.backend_error,
          viewProfileId: snapshot.config.selected_profile ?? snapshot.config.profiles[0]?.id ?? null,
        });
        for (const warning of snapshot.warnings) get().toast(warning, { kind: "warning" });
        if (import.meta.env.DEV) applyDevParams();
      } catch (error) {
        set({ fatal: String(error) });
      }
    },

    edit(change, coalesceKey) {
      const current = get().config;
      const next = change(current);
      if (next === current) return;
      const now = Date.now();
      const coalesce =
        coalesceKey !== undefined && coalesceKey === lastEditKey && now - lastEditAt < COALESCE_MS;
      lastEditKey = coalesceKey ?? null;
      lastEditAt = now;
      set((state) => ({
        config: next,
        past: coalesce ? state.past : [...state.past.slice(-HISTORY_LIMIT + 1), current],
        future: [],
      }));
      scheduleSave();
    },

    undo() {
      const { past, future, config } = get();
      if (past.length === 0) return;
      const previous = past[past.length - 1];
      lastEditKey = null;
      set({ config: previous, past: past.slice(0, -1), future: [config, ...future] });
      get().selectMapping(get().selectedMappingId);
      scheduleSave();
    },

    redo() {
      const { past, future, config } = get();
      if (future.length === 0) return;
      lastEditKey = null;
      set({ config: future[0], past: [...past, config], future: future.slice(1) });
      get().selectMapping(get().selectedMappingId);
      scheduleSave();
    },

    viewProfile(id) {
      const profile = get().config.profiles.find((p) => p.id === id);
      if (!profile) return;
      set({ viewProfileId: id, selectedMappingId: null, wizardMappingId: null, draft: null, search: "" });
      if (!profile.always_on && get().config.selected_profile !== id) {
        // Selecting is not an undoable edit.
        set((state) => ({ config: { ...state.config, selected_profile: id } }));
        scheduleSave();
      }
    },

    addProfile(name) {
      const profile = newProfile(name);
      get().edit((config) => ({ ...config, profiles: [...config.profiles, profile] }));
      get().viewProfile(profile.id);
      return profile.id;
    },

    updateProfile(id, change, coalesceKey) {
      get().edit((config) => mapProfile(config, id, change), coalesceKey);
    },

    removeProfile(id) {
      const { config, viewProfileId } = get();
      if (config.profiles.length <= 1) return;
      const remaining = config.profiles.filter((p) => p.id !== id);
      const fallback = remaining.find((p) => !p.always_on) ?? remaining[0];
      get().edit((c) => ({
        ...c,
        profiles: remaining,
        selected_profile: c.selected_profile === id ? fallback.id : c.selected_profile,
      }));
      if (viewProfileId === id) set({ viewProfileId: fallback.id, selectedMappingId: null });
    },

    cloneProfile(id) {
      const source = get().config.profiles.find((p) => p.id === id);
      if (!source) return;
      const copy = duplicateProfile(source, `${source.name} copy`);
      get().edit((config) => {
        const index = config.profiles.findIndex((p) => p.id === id);
        const profiles = config.profiles.slice();
        profiles.splice(index + 1, 0, copy);
        return { ...config, profiles };
      });
      get().viewProfile(copy.id);
    },

    moveProfile(id, toIndex) {
      get().edit((config) => {
        const from = config.profiles.findIndex((p) => p.id === id);
        return from < 0 ? config : { ...config, profiles: move(config.profiles, from, toIndex) };
      });
    },

    importProfiles(profiles) {
      if (profiles.length === 0) return;
      const fresh = profiles.map((p) => duplicateProfile(p, p.name));
      get().edit((config) => ({ ...config, profiles: [...config.profiles, ...fresh] }));
      get().viewProfile(fresh[0].id);
    },

    addMapping(mapping, options) {
      const profileId = get().viewProfileId;
      if (!profileId) return;
      get().edit((config) =>
        mapProfile(config, profileId, (profile) => {
          const mappings = profile.mappings.slice();
          mappings.splice(options?.index ?? mappings.length, 0, mapping);
          return { ...profile, mappings };
        }),
      );
      set({ selectedMappingId: mapping.id, wizardMappingId: options?.wizard ? mapping.id : null });
    },

    updateMapping(id, change, coalesceKey) {
      const draft = get().draft;
      if (draft && draft.id === id) {
        set({ draft: change(draft) });
        return;
      }
      get().edit((config) => mapMapping(config, id, change), coalesceKey ? `${id}:${coalesceKey}` : undefined);
    },

    removeMapping(id) {
      if (get().draft?.id === id) {
        set({ draft: null, selectedMappingId: null, wizardMappingId: null });
        return;
      }
      const before = get().config;
      const owner = profileOfMapping(before, id);
      if (!owner) return;
      get().edit((config) =>
        mapProfile(config, owner.id, (profile) => ({
          ...profile,
          mappings: profile.mappings.filter((m) => m.id !== id),
        })),
      );
      if (get().selectedMappingId === id) set({ selectedMappingId: null, wizardMappingId: null });
    },

    cloneMapping(id) {
      const owner = profileOfMapping(get().config, id);
      const source = owner?.mappings.find((m) => m.id === id);
      if (!owner || !source) return;
      const copy = duplicateMapping(source);
      get().edit((config) =>
        mapProfile(config, owner.id, (profile) => {
          const index = profile.mappings.findIndex((m) => m.id === id);
          const mappings = profile.mappings.slice();
          mappings.splice(index + 1, 0, copy);
          return { ...profile, mappings };
        }),
      );
      set({ selectedMappingId: copy.id });
    },

    moveMapping(id, toIndex) {
      const owner = profileOfMapping(get().config, id);
      if (!owner) return;
      get().edit((config) =>
        mapProfile(config, owner.id, (profile) => {
          const from = profile.mappings.findIndex((m) => m.id === id);
          return from < 0 ? profile : { ...profile, mappings: move(profile.mappings, from, toIndex) };
        }),
      );
    },

    moveMappingToProfile(id, profileId) {
      const owner = profileOfMapping(get().config, id);
      const mapping = owner?.mappings.find((m) => m.id === id);
      if (!owner || !mapping || owner.id === profileId) return;
      get().edit((config) => ({
        ...config,
        profiles: config.profiles.map((profile) => {
          if (profile.id === owner.id) {
            return { ...profile, mappings: profile.mappings.filter((m) => m.id !== id) };
          }
          if (profile.id === profileId) return { ...profile, mappings: [...profile.mappings, mapping] };
          return profile;
        }),
      }));
      if (get().selectedMappingId === id) set({ selectedMappingId: null });
    },

    selectMapping(id) {
      const draft = get().draft;
      if (draft && draft.id === id) return;
      const exists = id !== null && profileOfMapping(get().config, id) !== null;
      set((state) => ({
        selectedMappingId: exists ? id : null,
        wizardMappingId: exists && state.wizardMappingId === id ? id : null,
        // Moving on abandons a quick-add that never got as far as learning a control.
        draft: null,
      }));
    },

    startWizard() {
      if (!get().viewProfileId) return;
      const draft = newMapping();
      set({ draft, selectedMappingId: draft.id, wizardMappingId: draft.id, search: "" });
    },

    commitDraft() {
      const { draft, viewProfileId } = get();
      if (!draft || !viewProfileId) return;
      set({ draft: null });
      get().edit((config) =>
        mapProfile(config, viewProfileId, (profile) => ({ ...profile, mappings: [...profile.mappings, draft] })),
      );
    },

    finishWizard() {
      get().commitDraft();
      set({ wizardMappingId: null });
    },

    updateSettings(change, coalesceKey) {
      get().edit((config) => {
        const settings = change(config.settings);
        return settings === config.settings ? config : { ...config, settings };
      }, coalesceKey);
    },

    updateDevice(name, change, coalesceKey) {
      get().edit((config) => {
        const existing = config.devices.find((d) => d.name === name);
        const base: DeviceConfig = existing ?? { name, alias: "", enabled: true };
        const next = change(base);
        if (existing && next === existing) return config;
        const devices = existing
          ? config.devices.map((d) => (d.name === name ? next : d))
          : [...config.devices, next];
        return { ...config, devices };
      }, coalesceKey);
    },

    forgetDevice(name) {
      get().edit((config) => ({ ...config, devices: config.devices.filter((d) => d.name !== name) }));
    },

    setPaused(paused) {
      set((state) => ({ engine: { ...state.engine, paused } }));
      void backend?.setPaused(paused);
    },

    setPanel(panel) {
      set({ panel });
    },

    setMonitorOpen(open) {
      set({ monitorOpen: open });
    },

    setMonitorTab(tab) {
      set({ monitorTab: tab });
    },

    openVirtualController() {
      set({ monitorOpen: true, monitorTab: "virtual" });
    },

    setSearch(search) {
      set({ search });
    },

    toast(message, options) {
      const id = toastId++;
      set((state) => ({
        toasts: [...state.toasts.slice(-3), { id, kind: options?.kind ?? "info", message, action: options?.action }],
      }));
      const lifetime = options?.kind === "error" || options?.kind === "warning" ? 9000 : 4500;
      setTimeout(() => get().dismissToast(id), lifetime);
    },

    dismissToast(id) {
      set((state) => ({ toasts: state.toasts.filter((t) => t.id !== id) }));
    },
  };
});

/**
 * Development aid: put the UI into a given state from the URL, e.g.
 * `?select=m2&monitor=1`, `?wizard=1`, `?panel=settings`, `?theme=light`, `?profile=p2`.
 */
function applyDevParams(): void {
  const params = new URLSearchParams(window.location.search);
  const state = useApp.getState();
  const theme = params.get("theme");
  if (theme === "light" || theme === "dark") {
    useApp.setState({ config: { ...state.config, settings: { ...state.config.settings, theme } } });
  }
  const profile = params.get("profile");
  if (profile) useApp.setState({ viewProfileId: profile });
  const select = params.get("select");
  if (select) useApp.getState().selectMapping(select);
  if (params.has("wizard")) useApp.getState().startWizard();
  if (params.has("monitor")) useApp.setState({ monitorOpen: true });
  const panel = params.get("panel");
  if (panel === "settings" || panel === "devices") useApp.setState({ panel });
}

/** The backend, for imperative calls from components (learn, test, simulate, import/export). */
export async function api(): Promise<Backend> {
  return backend ?? (await getBackend());
}

// Convenience selectors ------------------------------------------------------------------------

export function useViewedProfile(): Profile | null {
  return useApp((state) => state.config.profiles.find((p) => p.id === state.viewProfileId) ?? null);
}

export function useSelectedMapping(): Mapping | null {
  return useApp((state) => {
    if (!state.selectedMappingId) return null;
    if (state.draft?.id === state.selectedMappingId) return state.draft;
    for (const profile of state.config.profiles) {
      const mapping = profile.mappings.find((m) => m.id === state.selectedMappingId);
      if (mapping) return mapping;
    }
    return null;
  });
}

/** Every layer name used anywhere in the config (for suggestions). */
export function useKnownLayers(): string[] {
  const config = useApp((state) => state.config);
  return useMemo(() => knownLayers(config), [config]);
}

export function useOctave(): number {
  return useApp((state) => state.config.settings.middle_c_octave ?? 4);
}
