import { Activity, AppWindow, Info, Layers, Pause, Pin, Play, Redo2, Settings, Settings2, TriangleAlert, Undo2 } from "lucide-react";

import type { Profile } from "@/bindings/Profile";
import { IconButton } from "@/components/ui/button";
import { Badge, KeyCombo } from "@/components/ui/misc";
import { Tooltip } from "@/components/ui/overlays";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";

import { LastEvent } from "../monitor/Monitor";

/** The big on/off control for the whole engine. */
function EngineSwitch() {
  const paused = useApp((state) => state.engine.paused);
  const learning = useApp((state) => state.engine.learning);
  const blocked = useApp((state) => !!state.backendError || !state.permissions.can_inject);
  const setPaused = useApp((state) => state.setPaused);
  const setPanel = useApp((state) => state.setPanel);

  if (blocked) {
    return (
      <Tooltip content="The system is not letting this app send keys. Click for details.">
        <button
          type="button"
          onClick={() => setPanel("settings")}
          className="flex h-[28px] items-center gap-1.5 rounded-full bg-danger-soft px-3 font-semibold text-danger hover:brightness-110"
        >
          <TriangleAlert size={14} />
          Can’t send keys
        </button>
      </Tooltip>
    );
  }
  return (
    <Tooltip
      content={
        paused
          ? "Paused: nothing is sent to your computer. Click to resume."
          : "Live: your mappings are sending keys. Click to pause."
      }
    >
      <button
        type="button"
        aria-pressed={!paused}
        onClick={() => setPaused(!paused)}
        className={cn(
          "flex h-[28px] items-center gap-2 rounded-full border pr-3 pl-2.5 font-semibold transition-colors",
          paused
            ? "border-warn/40 bg-warn-soft text-warn hover:bg-warn/20"
            : "border-midi-line bg-midi-soft text-midi hover:bg-midi/20",
        )}
      >
        {paused ? (
          <Play size={13} fill="currentColor" />
        ) : (
          <span className="relative flex size-[9px]">
            <span className={cn("absolute inset-0 rounded-full bg-midi", learning && "animate-ping")} />
            <span className="absolute inset-0 rounded-full bg-midi shadow-[0_0_8px_var(--midi)]" />
          </span>
        )}
        {paused ? "Paused" : learning ? "Learning" : "Active"}
        {!paused && <Pause size={12} className="opacity-60" />}
      </button>
    </Tooltip>
  );
}

export function TopBar({ profile, onEditProfile }: { profile: Profile | null; onEditProfile: (profile: Profile) => void }) {
  const canUndo = useApp((state) => state.past.length > 0);
  const canRedo = useApp((state) => state.future.length > 0);
  const undo = useApp((state) => state.undo);
  const redo = useApp((state) => state.redo);
  const setPanel = useApp((state) => state.setPanel);
  const effective = useApp((state) => state.engine.effective_profile);
  const autoSwitched = useApp((state) => state.engine.auto_switched);
  const effectiveName = useApp(
    (state) => state.config.profiles.find((p) => p.id === state.engine.effective_profile)?.name,
  );

  return (
    <header className="flex h-[48px] shrink-0 items-center gap-2.5 border-b bg-surface px-4">
      {profile && (
        <>
          <h1 className="truncate text-xl font-semibold">{profile.name}</h1>
          {profile.always_on && (
            <Badge tone="midi" title="These mappings work in every profile">
              <Pin size={11} /> always on
            </Badge>
          )}
          {profile.apps.length > 0 && (
            <Badge title="Activates automatically while one of these apps is in front">
              <AppWindow size={11} />
              {profile.apps
                .map((a) => a.process || a.title)
                .filter(Boolean)
                .slice(0, 2)
                .join(", ")}
              {profile.apps.length > 2 && ` +${profile.apps.length - 2}`}
            </Badge>
          )}
          <IconButton label="Profile settings" size="sm" onClick={() => onEditProfile(profile)}>
            <Settings2 size={14} />
          </IconButton>
          {autoSwitched && effective !== profile.id && effectiveName && (
            <Badge tone="warn" title="An app rule switched to another profile for the application in front">
              live right now: {effectiveName}
            </Badge>
          )}
        </>
      )}
      <div className="flex-1" />
      <IconButton label="Undo" shortcut="Ctrl+Z" disabled={!canUndo} onClick={undo}>
        <Undo2 size={16} />
      </IconButton>
      <IconButton label="Redo" shortcut="Ctrl+Shift+Z" disabled={!canRedo} onClick={redo}>
        <Redo2 size={16} />
      </IconButton>
      <div className="mx-1 h-[18px] w-px bg-line" />
      <EngineSwitch />
      <IconButton label="Settings" shortcut="Ctrl+," onClick={() => setPanel("settings")}>
        <Settings size={16} />
      </IconButton>
    </header>
  );
}

export function StatusBar() {
  const monitorOpen = useApp((state) => state.monitorOpen);
  const setMonitorOpen = useApp((state) => state.setMonitorOpen);
  const suspended = useApp(
    (state) => state.engine.focus_suspended && !state.engine.paused && state.config.settings.suspend_while_focused,
  );
  const layers = useApp((state) => state.engine.layers);
  const held = useApp((state) => state.engine.held_keys);
  const setPanel = useApp((state) => state.setPanel);

  return (
    <footer className="flex h-[28px] shrink-0 items-center gap-3 border-t bg-surface px-3 text-sm">
      <div className="flex min-w-0 flex-1 items-center gap-2">
        <Activity size={13} className="shrink-0 text-fg-faint" />
        <LastEvent />
      </div>
      {held.length > 0 && (
        <span className="flex shrink-0 items-center gap-1.5 text-fg-faint">
          holding <KeyCombo keys={held} small />
        </span>
      )}
      {layers.length > 0 && (
        <span className="flex shrink-0 items-center gap-1 text-accent">
          <Layers size={12} />
          {layers.join(", ")}
        </span>
      )}
      {suspended && (
        <Tooltip content="So your mappings cannot type into this editor. Switch to another window to use them — or turn this off in Settings.">
          <button
            type="button"
            onClick={() => setPanel("settings")}
            className="flex shrink-0 items-center gap-1 text-fg-faint hover:text-fg"
          >
            <Info size={12} />
            Output is held back while this window is in front
          </button>
        </Tooltip>
      )}
      <button
        type="button"
        aria-pressed={monitorOpen}
        onClick={() => setMonitorOpen(!monitorOpen)}
        className={cn(
          "flex h-[20px] shrink-0 items-center gap-1 rounded px-1.5 font-medium",
          monitorOpen ? "bg-accent-soft text-accent" : "text-fg-muted hover:bg-hover hover:text-fg",
        )}
      >
        Monitor
        <span className="text-2xs opacity-60">Ctrl+M</span>
      </button>
    </footer>
  );
}
