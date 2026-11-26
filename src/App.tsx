import { useEffect, useState } from "react";

import type { Profile } from "@/bindings/Profile";
import { Toasts } from "@/components/ui/misc";
import { TooltipProvider } from "@/components/ui/overlays";
import { MappingEditor } from "@/features/editor/MappingEditor";
import { MappingList } from "@/features/mappings/MappingList";
import { MonitorPanel } from "@/features/monitor/Monitor";
import { StatusBar, TopBar } from "@/features/shell/Bars";
import { DevicesDialog, ProfileDialog, SettingsDialog } from "@/features/shell/Dialogs";
import { Sidebar } from "@/features/shell/Sidebar";
import { isTauri } from "@/lib/platform";
import { useApp, useSelectedMapping, useViewedProfile } from "@/lib/store";

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
      } else if (mod && key === "d" && state.selectedMappingId && !typing) {
        event.preventDefault();
        state.cloneMapping(state.selectedMappingId);
      } else if (!mod && !event.altKey && key === "n" && !typing && state.panel === "none") {
        event.preventDefault();
        state.startWizard();
      } else if ((key === "delete" || key === "backspace") && state.selectedMappingId && !typing && state.panel === "none") {
        if ((event.target as HTMLElement | null)?.closest("[data-radix-popper-content-wrapper],[role=dialog]")) return;
        event.preventDefault();
        state.removeMapping(state.selectedMappingId);
        state.toast("Mapping deleted", { action: { label: "Undo", run: state.undo } });
      } else if ((key === "arrowdown" || key === "arrowup") && !typing && state.panel === "none") {
        const profile = state.config.profiles.find((p) => p.id === state.viewProfileId);
        if (!profile || profile.mappings.length === 0) return;
        if ((event.target as HTMLElement | null)?.closest("aside,[role=dialog],[role=menu],[role=listbox],[role=radiogroup]")) return;
        event.preventDefault();
        const index = profile.mappings.findIndex((m) => m.id === state.selectedMappingId);
        const next = index < 0 ? 0 : Math.min(profile.mappings.length - 1, Math.max(0, index + (key === "arrowdown" ? 1 : -1)));
        state.selectMapping(profile.mappings[next].id);
      }
    };
    window.addEventListener("keydown", onKeyDown);
    const onContextMenu = (event: MouseEvent) => {
      if (isTauri && !import.meta.env.DEV && !isTyping(event.target)) event.preventDefault();
    };
    window.addEventListener("contextmenu", onContextMenu);
    return () => {
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("contextmenu", onContextMenu);
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
          <div className="flex min-h-0 flex-1">
            {profile ? (
              <MappingList profile={profile} />
            ) : (
              <div className="flex flex-1 items-center justify-center text-fg-faint">Select or create a profile.</div>
            )}
            {mapping && <MappingEditor key={mapping.id} mapping={mapping} />}
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
