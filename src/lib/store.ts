// The main application store: the config being edited (with undo/redo and debounced saving),
// the engine's live status, and UI state. High-frequency data lives in `monitor.ts`.

import { useMemo } from "react";
import { create } from "zustand";

import type { AppInfo } from "@/bindings/AppInfo";
import type { Config } from "@/bindings/Config";
import type { DeviceConfig } from "@/bindings/DeviceConfig";
import type { EngineStatus } from "@/bindings/EngineStatus";
import type { Key } from "@/bindings/Key";
import type { Mapping } from "@/bindings/Mapping";
import type { Permissions } from "@/bindings/Permissions";
import type { PortStatus } from "@/bindings/PortStatus";
import type { Profile } from "@/bindings/Profile";
import type { RuntimeEvent } from "@/bindings/RuntimeEvent";
import type { Settings } from "@/bindings/Settings";
import type { Source } from "@/bindings/Source";

import { getBackend, type Backend } from "./api";
import { loadKeyboardLayout } from "./keys";
import {
  duplicateMapping,
  duplicateProfile,
  duplicateProfiles,
  isEmptyAction,
  knownLayers,
  newMapping,
  newProfile,
  primaryAction,
} from "./model";
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
  /** MIDI output ports (for "Send MIDI" and lights). */
  outputs: PortStatus[];
  permissions: Permissions;
  backendError: string;
  /** Keys the output backend on this system cannot send. */
  unsupportedKeys: Key[];

  past: Config[];
  future: Config[];
  saving: boolean;

  /** The profile shown in the main view (may be an always-on profile that is not "selected"). */
  viewProfileId: string | null;
  /** The mapping open in the editor. */
  selectedMappingId: string | null;
  /**
   * Mappings picked together (Ctrl / Shift click, Ctrl+A) for an action on all of them. Always
   * two or more, or empty: a single picked row is simply `selectedMappingId`.
   */
  multi: string[];
  /** Where a Shift selection starts from. */
  anchorId: string | null;
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
  /** Appends several mappings to the viewed profile as one undoable step. */
  addMappings(mappings: Mapping[]): void;
  updateMapping(id: string, change: (mapping: Mapping) => Mapping, coalesceKey?: string): void;
  removeMapping(id: string): void;
  cloneMapping(id: string): void;
  /**
   * Gives a control a second job on a layer: the mapping itself becomes "only while the layer is
   * off", and a copy "only while the layer is on" is added right below it and opened. With
   * `switchSource`, that control also becomes the layer's switch (the layer is on while it is held).
   */
  addLayerVariant(id: string, layer: string, switchSource?: Source | null): void;
  moveMapping(id: string, toIndex: number): void;
  moveMappingToProfile(id: string, profileId: string): void;
  /** Puts a copy of the mapping at the end of another profile (the original stays). */
  copyMappingToProfile(id: string, profileId: string): void;
  selectMapping(id: string | null): void;
  /** Ctrl+click: adds the mapping to the picked ones, or takes it out. */
  toggleSelect(id: string): void;
  /** Shift+click: picks everything from the anchor to this mapping, in the given (shown) order. */
  selectRange(toId: string, order: string[]): void;
  selectAll(ids: string[]): void;
  clearMulti(): void;
  /** The following act on several mappings as one undoable step. */
  removeMappings(ids: string[]): void;
  setMappingsEnabled(ids: string[], enabled: boolean): void;
  cloneMappings(ids: string[]): void;
  moveMappingsToProfile(ids: string[], profileId: string): void;
  copyMappingsToProfile(ids: string[], profileId: string): void;
  /** Opens the quick-add flow with a fresh draft mapping. */
  startWizard(): void;
  /**
   * Opens the quick-add flow for a control that is already known ("map it" on a message
   * nothing reacts to): the mapping exists right away and only waits for its action.
   */
  startWizardWith(source: Source): void;
  /** Where the quick add that is open began: listening for a control, or with one in hand. */
  wizardStage: "learn" | "action";
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
  latency: null,
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
/** When each error message was last shown, so a recurring one does not spam. */
const recentErrors = new Map<string, number>();

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
          if (e.type === "error") {
            // Problems the user should hear about even with the monitor closed (once each).
            const now = Date.now();
            if (now - (recentErrors.get(e.message) ?? 0) > 30_000) {
              recentErrors.set(e.message, now);
              get().toast(e.message, { kind: "error" });
            }
            continue;
          }
          if (e.type === "fired" && e.outcome.type === "failed") {
            // The system refused something (an elevated window, a missing permission, a key this
            // platform lacks): worth saying out loud, but not for every repeat.
            // The engine words MIDI firings as "send CC 7 = 90 …" and "light on: …" already.
            const message = e.description.startsWith("send ")
              ? `Could not ${e.description}: ${e.outcome.error}`
              : e.description.startsWith("light ")
                ? `The controller's ${e.description.replace(": ", " (")}) could not be sent: ${e.outcome.error}`
                : `Could not send ${e.description}: ${e.outcome.error}`;
            const now = Date.now();
            if (now - (recentErrors.get(e.outcome.error) ?? 0) > 30_000) {
              recentErrors.set(e.outcome.error, now);
              // The backend words elevation problems with this phrase (ELEVATION_REQUIRED).
              const needsAdmin = e.outcome.error.includes("as administrator") && !get().info.elevated;
              get().toast(message, {
                kind: "warning",
                action: needsAdmin
                  ? { label: "Restart as administrator", run: () => void restartAsAdmin() }
                  : undefined,
              });
            }
            continue;
          }
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
      case "outputs":
        set({ outputs: event.outputs });
        break;
      case "permissions":
        set({
          permissions: event.permissions,
          backendError: event.backend_error,
          unsupportedKeys: event.unsupported_keys ?? [],
        });
        break;
      case "config": {
        // The config changed behind our back (the file was edited by hand, a profile was
        // switched from the tray, …): adopt it and keep the view pointing at things that exist.
        const state = get();
        const config = event.config;
        const viewed = config.profiles.some((p) => p.id === state.viewProfileId)
          ? state.viewProfileId
          : (config.selected_profile ?? config.profiles[0]?.id ?? null);
        const selected =
          state.selectedMappingId && profileOfMapping(config, state.selectedMappingId) ? state.selectedMappingId : null;
        const edited = !deepEqual({ ...config, selected_profile: null }, { ...state.config, selected_profile: null });
        set({
          config,
          past: [],
          future: [],
          viewProfileId: viewed,
          selectedMappingId: selected,
          multi: [],
          wizardMappingId: null,
          draft: null,
        });
        if (edited) get().toast("The configuration file changed on disk and was reloaded.", { kind: "info" });
        break;
      }
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

  /**
   * A quick-add that learned a control but was abandoned before it got an action would leave a
   * "does nothing" mapping in the list. Walking away from the wizard removes it again.
   */
  const discardUnfinishedWizard = () => {
    const { wizardMappingId, config } = get();
    if (!wizardMappingId) return;
    const owner = profileOfMapping(config, wizardMappingId);
    const mapping = owner?.mappings.find((m) => m.id === wizardMappingId);
    if (!owner || !mapping || !isEmptyAction(primaryAction(mapping.behavior))) return;
    set({ wizardMappingId: null });
    get().edit((c) =>
      mapProfile(c, owner.id, (profile) => ({
        ...profile,
        mappings: profile.mappings.filter((m) => m.id !== wizardMappingId),
      })),
    );
  };

  return {
    ready: false,
    fatal: null,
    info: { version: "", platform: "", config_path: "", wayland: false, elevated: false },
    config: EMPTY_CONFIG,
    engine: EMPTY_STATUS,
    ports: [],
    outputs: [],
    permissions: { can_inject: true, can_detect_foreground: true, hint: "" },
    backendError: "",
    unsupportedKeys: [],
    past: [],
    future: [],
    saving: false,
    viewProfileId: null,
    selectedMappingId: null,
    multi: [],
    anchorId: null,
    wizardStage: "learn",
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
          outputs: snapshot.outputs ?? [],
          permissions: snapshot.permissions,
          backendError: snapshot.backend_error,
          unsupportedKeys: snapshot.unsupported_keys ?? [],
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
      discardUnfinishedWizard();
      set({ viewProfileId: id, selectedMappingId: null, multi: [], anchorId: null, wizardMappingId: null, draft: null, search: "" });
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
      const taken = new Set(get().config.profiles.map((p) => p.name));
      const names = profiles.map((p) => {
        let name = p.name;
        for (let n = 2; taken.has(name); n++) name = `${p.name} (${n})`;
        taken.add(name);
        return name;
      });
      // Together, so that profiles which switch to each other still do after the import.
      const fresh = duplicateProfiles(profiles, names);
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

    addMappings(mappings) {
      const profileId = get().viewProfileId;
      if (!profileId || mappings.length === 0) return;
      get().edit((config) =>
        mapProfile(config, profileId, (profile) => ({ ...profile, mappings: [...profile.mappings, ...mappings] })),
      );
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

    addLayerVariant(id, layer, switchSource) {
      const owner = profileOfMapping(get().config, id);
      const source = owner?.mappings.find((m) => m.id === id);
      if (!owner || !source || !layer) return;
      const others = source.conditions.filter((c) => !(c.type === "layer" && c.layer === layer));
      const base: Mapping = { ...source, conditions: [...others, { type: "layer", layer, active: false }] };
      const variant: Mapping = {
        ...duplicateMapping(source),
        name: source.name ? `${source.name} (${layer})` : "",
        conditions: [...others, { type: "layer", layer, active: true }],
      };
      const layerSwitch = switchSource
        ? [newMapping(switchSource, { type: "hold", target: { type: "layer", layer }, auto_repeat: true })]
        : [];
      get().edit((config) =>
        mapProfile(config, owner.id, (profile) => {
          const index = profile.mappings.findIndex((m) => m.id === id);
          const mappings = profile.mappings.slice();
          mappings.splice(index, 1, base, variant, ...layerSwitch);
          return { ...profile, mappings };
        }),
      );
      set({ selectedMappingId: variant.id });
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
      // A selection of several is about the profile in view; it does not follow a mapping out.
      if (get().multi.includes(id)) set({ multi: [] });
    },

    copyMappingToProfile(id, profileId) {
      const owner = profileOfMapping(get().config, id);
      const mapping = owner?.mappings.find((m) => m.id === id);
      if (!owner || !mapping) return;
      const copy = duplicateMapping(mapping);
      get().edit((config) =>
        mapProfile(config, profileId, (profile) => ({ ...profile, mappings: [...profile.mappings, copy] })),
      );
    },

    selectMapping(id) {
      const draft = get().draft;
      if (draft && draft.id === id) return;
      if (get().wizardMappingId !== id) discardUnfinishedWizard();
      const exists = id !== null && profileOfMapping(get().config, id) !== null;
      set((state) => ({
        selectedMappingId: exists ? id : null,
        multi: [],
        anchorId: exists ? id : state.anchorId,
        wizardMappingId: exists && state.wizardMappingId === id ? id : null,
        // Moving on abandons a quick-add that never got as far as learning a control.
        draft: null,
      }));
    },

    toggleSelect(id) {
      const { multi, selectedMappingId, draft } = get();
      const current = multi.length > 0 ? multi : selectedMappingId && draft?.id !== selectedMappingId ? [selectedMappingId] : [];
      const next = current.includes(id) ? current.filter((other) => other !== id) : [...current, id];
      if (next.length <= 1) {
        get().selectMapping(next[0] ?? null);
        return;
      }
      discardUnfinishedWizard();
      set({ multi: next, anchorId: id, selectedMappingId: null, wizardMappingId: null, draft: null });
    },

    selectRange(toId, order) {
      const { anchorId, selectedMappingId } = get();
      const from = anchorId ?? selectedMappingId ?? toId;
      const a = order.indexOf(from);
      const b = order.indexOf(toId);
      const range = a < 0 || b < 0 ? [toId] : order.slice(Math.min(a, b), Math.max(a, b) + 1);
      if (range.length <= 1) {
        get().selectMapping(toId);
        return;
      }
      discardUnfinishedWizard();
      set({ multi: range, anchorId: from, selectedMappingId: null, wizardMappingId: null, draft: null });
    },

    selectAll(ids) {
      if (ids.length <= 1) {
        get().selectMapping(ids[0] ?? null);
        return;
      }
      discardUnfinishedWizard();
      set({ multi: ids, selectedMappingId: null, wizardMappingId: null, draft: null });
    },

    clearMulti() {
      set({ multi: [] });
    },

    removeMappings(ids) {
      const drop = new Set(ids);
      get().edit((config) => ({
        ...config,
        profiles: config.profiles.map((profile) =>
          profile.mappings.some((m) => drop.has(m.id))
            ? { ...profile, mappings: profile.mappings.filter((m) => !drop.has(m.id)) }
            : profile,
        ),
      }));
      set((state) => ({
        multi: [],
        selectedMappingId: state.selectedMappingId && drop.has(state.selectedMappingId) ? null : state.selectedMappingId,
      }));
    },

    setMappingsEnabled(ids, enabled) {
      const pick = new Set(ids);
      get().edit((config) => ({
        ...config,
        profiles: config.profiles.map((profile) =>
          profile.mappings.some((m) => pick.has(m.id) && m.enabled !== enabled)
            ? { ...profile, mappings: profile.mappings.map((m) => (pick.has(m.id) ? { ...m, enabled } : m)) }
            : profile,
        ),
      }));
    },

    cloneMappings(ids) {
      const pick = new Set(ids);
      const copies: string[] = [];
      get().edit((config) => ({
        ...config,
        profiles: config.profiles.map((profile) => {
          const picked = profile.mappings.filter((m) => pick.has(m.id));
          if (picked.length === 0) return profile;
          // The copies go together, right after the last of the originals.
          const last = profile.mappings.findIndex((m) => m.id === picked[picked.length - 1].id);
          const duplicates = picked.map(duplicateMapping);
          duplicates.forEach((copy) => copies.push(copy.id));
          return {
            ...profile,
            mappings: [...profile.mappings.slice(0, last + 1), ...duplicates, ...profile.mappings.slice(last + 1)],
          };
        }),
      }));
      if (copies.length > 1) set({ multi: copies, selectedMappingId: null, wizardMappingId: null });
      else if (copies.length === 1) get().selectMapping(copies[0]);
    },

    moveMappingsToProfile(ids, profileId) {
      const pick = new Set(ids);
      const config = get().config;
      const moving = config.profiles
        .filter((profile) => profile.id !== profileId)
        .flatMap((profile) => profile.mappings.filter((m) => pick.has(m.id)));
      if (moving.length === 0) return;
      const gone = new Set(moving.map((m) => m.id));
      get().edit((current) => ({
        ...current,
        profiles: current.profiles.map((profile) => {
          if (profile.id === profileId) return { ...profile, mappings: [...profile.mappings, ...moving] };
          return profile.mappings.some((m) => gone.has(m.id))
            ? { ...profile, mappings: profile.mappings.filter((m) => !gone.has(m.id)) }
            : profile;
        }),
      }));
      set({ multi: [], selectedMappingId: null });
    },

    copyMappingsToProfile(ids, profileId) {
      const pick = new Set(ids);
      const copies = get()
        .config.profiles.flatMap((profile) => profile.mappings.filter((m) => pick.has(m.id)))
        .map(duplicateMapping);
      if (copies.length === 0) return;
      get().edit((config) =>
        mapProfile(config, profileId, (profile) => ({ ...profile, mappings: [...profile.mappings, ...copies] })),
      );
    },

    startWizard() {
      if (!get().viewProfileId) return;
      discardUnfinishedWizard();
      const draft = newMapping();
      set({ draft, selectedMappingId: draft.id, wizardMappingId: draft.id, wizardStage: "learn", multi: [], search: "" });
    },

    startWizardWith(source) {
      const profileId = get().viewProfileId;
      if (!profileId) return;
      discardUnfinishedWizard();
      const mapping = newMapping(source);
      get().edit((config) =>
        mapProfile(config, profileId, (profile) => ({ ...profile, mappings: [...profile.mappings, mapping] })),
      );
      set({
        draft: null,
        selectedMappingId: mapping.id,
        wizardMappingId: mapping.id,
        wizardStage: "action",
        multi: [],
        search: "",
      });
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
        const base: DeviceConfig = existing ?? { name, alias: "", enabled: true, forward_to: null };
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
        // The same message again takes the place of the one that is showing instead of stacking.
        toasts: [
          ...state.toasts.filter((toast) => toast.message !== message).slice(-3),
          { id, kind: options?.kind ?? "info", message, action: options?.action },
        ],
      }));
      const lifetime = options?.kind === "error" || options?.kind === "warning" || options?.action ? 9000 : 4500;
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

/** Restarts the app elevated; if the user declines the prompt, says so. */
export async function restartAsAdmin(): Promise<void> {
  try {
    await (await api()).restartAsAdmin();
  } catch (error) {
    useApp.getState().toast(String(error), { kind: "warning" });
  }
}

/** What Ctrl+C puts on the clipboard for a mapping: plain JSON that can be pasted back, shared or edited. */
export function mappingsToText(mappings: Mapping[]): string {
  return JSON.stringify({ m2k: "mappings", mappings }, null, 2);
}

/**
 * The mappings in a piece of text, if it holds any: what `mappingsToText` wrote, a bare mapping,
 * or a list of them (hand-written ones included — the backend's forgiving loader completes
 * them). `null` when the text is something else.
 */
export async function mappingsFromText(text: string): Promise<Mapping[] | null> {
  let data: unknown;
  try {
    data = JSON.parse(text);
  } catch {
    return null;
  }
  const looksLikeMapping = (item: unknown) => typeof item === "object" && item !== null && "source" in item && "behavior" in item;
  const list = Array.isArray(data)
    ? data
    : typeof data === "object" && data !== null && Array.isArray((data as { mappings?: unknown }).mappings)
      ? (data as { mappings: unknown[] }).mappings
      : looksLikeMapping(data)
        ? [data]
        : null;
  if (!list || list.length === 0 || !list.every(looksLikeMapping)) return null;
  const profiles = await (await api()).parseProfiles(JSON.stringify({ profiles: [{ name: "clipboard", mappings: list }] }));
  const parsed = profiles[0]?.mappings ?? [];
  return parsed.length > 0 ? parsed.map(duplicateMapping) : null;
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
