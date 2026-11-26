import { useEffect, useState } from "react";

import type { Profile } from "@/bindings/Profile";
import { Toasts } from "@/components/ui/misc";
import { TooltipProvider } from "@/components/ui/overlays";
import { MappingEditor } from "@/features/editor/MappingEditor";
import { bulk, BulkPanel, pickedMappings } from "@/features/mappings/BulkPanel";
import { MappingList } from "@/features/mappings/MappingList";
import { MonitorPanel } from "@/features/monitor/Monitor";
import { StatusBar, TopBar } from "@/features/shell/Bars";
import { DevicesDialog, ProfileDialog, SettingsDialog } from "@/features/shell/Dialogs";
import { Notices } from "@/features/shell/Notices";
import { Sidebar } from "@/features/shell/Sidebar";
import { isTauri } from "@/lib/platform";
import { mappingsFromText, mappingsToText, profileOfMapping, useApp, useSelectedMapping, useViewedProfile } from "@/lib/store";

function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName);
}

/** Applies the theme setting to the document. */
function useTheme() {
  const theme = useApp((state) => state.config.settings.theme ?? "system");
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: light)");
    const apply = () => {
      const resolved = theme === "system" ? (media.matches ? "light" : "dark") : theme;
      document.documentElement.dataset.theme = resolved;
    };
    apply();
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme]);
}

/** The mapping whose row holds the keyboard focus, if any. */
function focusedMappingId(): string | null {
  const row = (document.activeElement as HTMLElement | null)?.closest<HTMLElement>("[data-mapping-id]");
  return row?.dataset.mappingId ?? null;
}

/** Moves the keyboard focus to a mapping's row (once it is rendered) and brings it into view. */
function focusMappingRow(id: string | null | undefined) {
  if (!id) return;
  requestAnimationFrame(() => {
    const row = document.querySelector<HTMLElement>(`[data-mapping-id="${CSS.escape(id)}"]`);
    row?.focus({ preventScroll: true });
    row?.scrollIntoView({ block: "nearest" });
  });
}

/** App-wide keyboard shortcuts. Key capture fields stop propagation, so they are never in conflict. */
function useShortcuts() {
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      const state = useApp.getState();
      const mod = event.ctrlKey || event.metaKey;
      const typing = isTyping(event.target);
      const key = event.key.toLowerCase();

      // The shell is an app, not a web page: no reload / print / find-in-page in production.
      if (isTauri && !import.meta.env.DEV) {
        if (key === "f5" || key === "f3" || key === "f7" || (mod && ["r", "p", "g", "u", "j"].includes(key))) {
          event.preventDefault();
        }
      }

      const overlayOpen = () =>
        document.querySelector('[data-radix-popper-content-wrapper], [role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]') !== null;

      // Escape lets go of a selection of several mappings...
      if (key === "escape" && !event.defaultPrevented && state.multi.length > 0 && state.panel === "none" && !typing) {
        if (!overlayOpen()) state.clearMulti();
        return;
      }

      // Escape closes the editor from anywhere, unless something on top of it wants the key.
      if (key === "escape" && !event.defaultPrevented && state.selectedMappingId && state.panel === "none") {
        const overlay = document.querySelector(
          '[data-radix-popper-content-wrapper], [role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]',
        );
        if (!overlay) {
          if (typing) (event.target as HTMLElement).blur();
          else state.selectMapping(null);
        }
        return;
      }

      if (mod && key === "z" && !typing) {
        event.preventDefault();
        if (event.shiftKey) state.redo();
        else state.undo();
      } else if (mod && key === "y" && !typing) {
        event.preventDefault();
        state.redo();
      } else if (mod && key === "m") {
        event.preventDefault();
        state.setMonitorOpen(!state.monitorOpen);
      } else if (mod && key === ",") {
        event.preventDefault();
        state.setPanel("settings");
      } else if (mod && key === "f") {
        event.preventDefault();
        document.querySelector<HTMLInputElement>('input[aria-label="Search mappings"]')?.focus();
      } else if (mod && key === "a" && !typing && state.panel === "none" && !overlayOpen()) {
        // Ctrl+A picks every mapping that is shown.
        const shown = Array.from(
          document.querySelectorAll<HTMLElement>('[role="row"][data-mapping-id]'),
          (row) => row.dataset.mappingId!,
        );
        if (shown.length === 0) return;
        event.preventDefault();
        state.selectAll(shown);
      } else if (mod && key === "d" && state.multi.length > 1 && !typing) {
        event.preventDefault();
        bulk.duplicate(state.multi);
      } else if (mod && key === "d" && (focusedMappingId() ?? state.selectedMappingId) && !typing) {
        event.preventDefault();
        state.cloneMapping((focusedMappingId() ?? state.selectedMappingId)!);
        focusMappingRow(useApp.getState().selectedMappingId);
      } else if (!mod && document.querySelector('[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"]')) {
        // A dialog or menu is open: single-key shortcuts belong to it, not to the list behind it.
        return;
      } else if (!mod && !event.altKey && key === "n" && !typing && state.panel === "none") {
        event.preventDefault();
        state.startWizard();
      } else if ((key === "delete" || key === "backspace") && !typing && state.panel === "none") {
        // Deleting a mapping needs clear intent: the key must come from a mapping's row, or from
        // nowhere in particular (nothing focused). A Backspace that a control of the editor has
        // used for itself — clearing a shortcut field, say — must never take the mapping with it.
        if (event.defaultPrevented) return;
        const target = event.target as HTMLElement | null;
        const onRow = focusedMappingId();
        const nowhere = !target || target === document.body || target === document.documentElement;
        if (!onRow && !nowhere) return;
        if (state.multi.length > 1) {
          event.preventDefault();
          bulk.remove(state.multi);
          return;
        }
        // The row with the focus ring is the one that goes; the focus moves on to its neighbor.
        const id = onRow ?? state.selectedMappingId;
        if (!id) return;
        event.preventDefault();
        const siblings = state.config.profiles.find((p) => p.mappings.some((m) => m.id === id))?.mappings ?? [];
        const at = siblings.findIndex((m) => m.id === id);
        state.removeMapping(id);
        state.toast("Mapping deleted", { action: { label: "Undo", run: state.undo } });
        focusMappingRow((siblings[at + 1] ?? siblings[at - 1])?.id);
      } else if ((key === "arrowdown" || key === "arrowup") && !typing && state.panel === "none") {
        const profile = state.config.profiles.find((p) => p.id === state.viewProfileId);
        if (!profile || profile.mappings.length === 0) return;
        if ((event.target as HTMLElement | null)?.closest("aside,[role=dialog],[role=menu],[role=listbox],[role=radiogroup]")) return;
        event.preventDefault();
        // Start from the row with the focus ring; selection and focus then travel together.
        const from = focusedMappingId() ?? state.selectedMappingId;
        const index = profile.mappings.findIndex((m) => m.id === from);
        const step = key === "arrowdown" ? 1 : -1;
        if (event.altKey) {
          // Alt+arrows carry the mapping along (not while a search hides part of the list).
          if (index < 0 || state.search !== "") return;
          const to = index + step;
          if (to < 0 || to >= profile.mappings.length) return;
          state.moveMapping(profile.mappings[index].id, to);
          focusMappingRow(profile.mappings[index].id);
          return;
        }
        // Walk the rows that are actually shown (a search may hide some of the profile's mappings).
        const shown = Array.from(
          document.querySelectorAll<HTMLElement>('[role="row"][data-mapping-id]'),
          (row) => row.dataset.mappingId!,
        );
        if (shown.length === 0) return;
        const at = shown.indexOf(from ?? "");
        const next = shown[at < 0 ? 0 : Math.min(shown.length - 1, Math.max(0, at + step))];
        // Shift+arrows grow or shrink a selection of several, from where it started.
        if (event.shiftKey) state.selectRange(next, shown);
        else state.selectMapping(next);
        focusMappingRow(next);
      }
    };
    window.addEventListener("keydown", onKeyDown);

    // Ctrl+C / Ctrl+V move mappings around as plain JSON text: between profiles, into a chat or
    // a text editor, and back (hand-written JSON pastes too). Text fields keep the clipboard.
    const onCopy = (event: ClipboardEvent) => {
      const state = useApp.getState();
      if (isTyping(event.target) || window.getSelection()?.toString() || state.panel !== "none") return;
      const id = focusedMappingId() ?? state.selectedMappingId;
      const mapping = id ? profileOfMapping(state.config, id)?.mappings.find((m) => m.id === id) : null;
      const copied = state.multi.length > 1 ? pickedMappings(state.multi) : mapping ? [mapping] : [];
      if (copied.length === 0 || !event.clipboardData) return;
      event.clipboardData.setData("text/plain", mappingsToText(copied));
      event.preventDefault();
      state.toast(
        copied.length === 1
          ? "Mapping copied. Paste it into another profile — or anywhere, it is plain text."
          : `${copied.length} mappings copied. Paste them into another profile — or anywhere, they are plain text.`,
      );
    };
    const onPaste = (event: ClipboardEvent) => {
      const state = useApp.getState();
      if (isTyping(event.target) || state.panel !== "none" || !state.viewProfileId) return;
      if (document.querySelector('[role="dialog"], [role="alertdialog"]')) return;
      const text = event.clipboardData?.getData("text/plain") ?? "";
      if (!/^\s*[[{]/.test(text)) {
        if (text.trim()) state.toast("The clipboard holds no mappings — copy one with Ctrl+C first.");
        return;
      }
      event.preventDefault();
      void mappingsFromText(text)
        .then((mappings) => {
          if (!mappings) {
            state.toast("The clipboard holds no mappings — copy one with Ctrl+C first.");
            return;
          }
          state.addMappings(mappings);
          state.selectMapping(mappings[0].id);
          focusMappingRow(mappings[0].id);
          state.toast(mappings.length === 1 ? "Mapping pasted" : `${mappings.length} mappings pasted`, {
            kind: "success",
            action: { label: "Undo", run: state.undo },
          });
        })
        .catch((error) => state.toast(`Could not paste: ${String(error)}`, { kind: "error" }));
    };
    document.addEventListener("copy", onCopy);
    document.addEventListener("paste", onPaste);
    const onContextMenu = (event: MouseEvent) => {
      if (isTauri && !import.meta.env.DEV && !isTyping(event.target)) event.preventDefault();
    };
    window.addEventListener("contextmenu", onContextMenu);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("contextmenu", onContextMenu);
      document.removeEventListener("copy", onCopy);
      document.removeEventListener("paste", onPaste);
    };
  }, []);
}

export function App() {
  const ready = useApp((state) => state.ready);
  const fatal = useApp((state) => state.fatal);
  const init = useApp((state) => state.init);
  const panel = useApp((state) => state.panel);
  const setPanel = useApp((state) => state.setPanel);
  const monitorOpen = useApp((state) => state.monitorOpen);
  const profile = useViewedProfile();
  const mapping = useSelectedMapping();
  const multi = useApp((state) => state.multi);
  const [editingProfile, setEditingProfile] = useState<Profile | null>(null);

  useEffect(() => {
    void init();
  }, [init]);
  useTheme();
  useShortcuts();

  if (fatal) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-2 p-8 text-center">
        <div className="text-xl font-semibold">MIDI 2 Keyboard could not start</div>
        <div className="selectable max-w-[560px] text-fg-muted">{fatal}</div>
      </div>
    );
  }
  if (!ready) {
    return <div className="flex h-full items-center justify-center text-fg-faint">Starting…</div>;
  }

  return (
    <TooltipProvider delayDuration={350} skipDelayDuration={200}>
      <div className="flex h-full animate-fade-in">
        <Sidebar onEditProfile={setEditingProfile} />
        <div className="flex min-w-0 flex-1 flex-col">
          <TopBar profile={profile} onEditProfile={setEditingProfile} />
          <Notices />
          <div className="relative flex min-h-0 flex-1">
            {profile ? (
              <MappingList profile={profile} />
            ) : (
              <div className="flex flex-1 items-center justify-center text-fg-faint">Select or create a profile.</div>
            )}
            {multi.length > 1 ? <BulkPanel ids={multi} /> : mapping && <MappingEditor key={mapping.id} mapping={mapping} />}
          </div>
          {monitorOpen && <MonitorPanel />}
          <StatusBar />
        </div>
      </div>
      <SettingsDialog open={panel === "settings"} onOpenChange={(open) => setPanel(open ? "settings" : "none")} />
      <DevicesDialog open={panel === "devices"} onOpenChange={(open) => setPanel(open ? "devices" : "none")} />
      <ProfileDialog profile={editingProfile} onClose={() => setEditingProfile(null)} />
      <Toasts />
    </TooltipProvider>
  );
}
