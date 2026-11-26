import {
  AlertTriangle,
  AppWindow,
  ArrowDown,
  ArrowUp,
  Copy,
  Download,
  FolderOpen,
  MoreHorizontal,
  Pencil,
  Pin,
  Plus,
  Settings2,
  Trash2,
  Upload,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";

import type { PortStatus } from "@/bindings/PortStatus";
import type { Profile } from "@/bindings/Profile";
import { IconButton } from "@/components/ui/button";
import {
  ConfirmDialog,
  Menu,
  MenuContent,
  MenuItem,
  MenuLabel,
  MenuSeparator,
  MenuTrigger,
  Tooltip,
} from "@/components/ui/overlays";
import { deviceLabel } from "@/lib/describe";
import { EXAMPLES, importExample } from "@/lib/examples";
import { onDeviceActivity } from "@/lib/monitor";
import { api, useApp } from "@/lib/store";
import { cn } from "@/lib/utils";

function ProfileItem({ profile, onEdit }: { profile: Profile; onEdit: (profile: Profile) => void }) {
  const viewed = useApp((state) => state.viewProfileId === profile.id);
  const live = useApp(
    (state) => state.engine.effective_profile === profile.id || profile.always_on,
  );
  const auto = useApp((state) => state.engine.auto_switched && state.engine.effective_profile === profile.id);
  const viewProfile = useApp((state) => state.viewProfile);
  const updateProfile = useApp((state) => state.updateProfile);
  const cloneProfile = useApp((state) => state.cloneProfile);
  const removeProfile = useApp((state) => state.removeProfile);
  const onlyOne = useApp((state) => state.config.profiles.length <= 1);
  const count = useApp((state) => state.config.profiles.length);
  const index = useApp((state) => state.config.profiles.findIndex((p) => p.id === profile.id));
  const moveProfile = useApp((state) => state.moveProfile);
  const toast = useApp((state) => state.toast);
  const [renaming, setRenaming] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (renaming) inputRef.current?.select();
  }, [renaming]);

  const exportProfile = async () => {
    const backend = await api();
    const ok = await backend.exportProfiles([profile], `${profile.name}.m2k.json`);
    if (ok) toast(`Exported “${profile.name}”`, { kind: "success" });
  };

  return (
    <div
      role="button"
      tabIndex={0}
      aria-current={viewed}
      onClick={() => viewProfile(profile.id)}
      onDoubleClick={() => setRenaming(true)}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter") viewProfile(profile.id);
        if (e.key === "F2") setRenaming(true);
      }}
      className={cn(
        "group flex h-[30px] cursor-pointer items-center gap-2 rounded-md pr-1 pl-2 outline-hidden transition-colors",
        viewed ? "bg-active text-fg" : "text-fg-muted hover:bg-hover hover:text-fg focus-visible:bg-hover",
      )}
    >
      <Tooltip
        content={
          profile.always_on
            ? "Always on: these mappings work in every profile"
            : auto
              ? "Live — switched to automatically for the app in front"
              : live
                ? "Live — this profile's mappings are active"
                : null
        }
        side="right"
      >
        <span className="flex w-[14px] shrink-0 justify-center">
          {profile.always_on ? (
            <Pin size={12} className="text-midi" />
          ) : (
            <span className={cn("size-[7px] rounded-full", live ? "bg-midi shadow-[0_0_6px_var(--midi)]" : "bg-line-strong")} />
          )}
        </span>
      </Tooltip>
      {renaming ? (
        <input
          ref={inputRef}
          defaultValue={profile.name}
          aria-label="Profile name"
          onClick={(e) => e.stopPropagation()}
          onBlur={(e) => {
            const name = e.target.value.trim();
            if (name && name !== profile.name) updateProfile(profile.id, (p) => ({ ...p, name }));
            setRenaming(false);
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter") e.currentTarget.blur();
            if (e.key === "Escape") setRenaming(false);
          }}
          className="h-[22px] min-w-0 flex-1 rounded border border-accent bg-app px-1 text-fg outline-hidden"
        />
      ) : (
        <span className="min-w-0 flex-1 truncate" title={profile.name}>
          {profile.name}
        </span>
      )}
      {profile.apps.length > 0 && !renaming && (
        <Tooltip content={`Auto-activates for: ${profile.apps.map((a) => a.process || a.title).join(", ")}`} side="right">
          <AppWindow size={12} className="shrink-0 text-fg-faint" />
        </Tooltip>
      )}
      <span className="shrink-0 text-sm text-fg-faint tabular-nums group-hover:hidden">{profile.mappings.length || ""}</span>
      <span className="hidden shrink-0 group-hover:flex" onClick={(e) => e.stopPropagation()}>
        <Menu>
          <MenuTrigger asChild>
            <IconButton label="Profile options" size="sm">
              <MoreHorizontal size={14} />
            </IconButton>
          </MenuTrigger>
          <MenuContent align="start">
            <MenuItem icon={<Settings2 size={14} />} onSelect={() => onEdit(profile)}>
              Profile settings…
            </MenuItem>
            <MenuItem icon={<Pencil size={14} />} shortcut="F2" onSelect={() => setRenaming(true)}>
              Rename
            </MenuItem>
            <MenuItem icon={<Copy size={14} />} onSelect={() => cloneProfile(profile.id)}>
              Duplicate
            </MenuItem>
            <MenuItem icon={<Upload size={14} />} onSelect={exportProfile}>
              Export…
            </MenuItem>
            <MenuSeparator />
            <MenuItem icon={<ArrowUp size={14} />} disabled={index === 0} onSelect={() => moveProfile(profile.id, index - 1)}>
              Move up
            </MenuItem>
            <MenuItem icon={<ArrowDown size={14} />} disabled={index === count - 1} onSelect={() => moveProfile(profile.id, index + 1)}>
              Move down
            </MenuItem>
            <MenuSeparator />
            <MenuItem tone="danger" icon={<Trash2 size={14} />} disabled={onlyOne} onSelect={() => setConfirming(true)}>
              Delete
            </MenuItem>
          </MenuContent>
        </Menu>
      </span>
      <ConfirmDialog
        open={confirming}
        onOpenChange={setConfirming}
        title={`Delete “${profile.name}”?`}
        description={
          profile.mappings.length
            ? `Its ${profile.mappings.length} mapping${profile.mappings.length === 1 ? "" : "s"} will be deleted too. You can undo this with Ctrl+Z.`
            : "You can undo this with Ctrl+Z."
        }
        confirmLabel="Delete profile"
        onConfirm={() => removeProfile(profile.id)}
      />
    </div>
  );
}

function DeviceLed({ name, connected }: { name: string; connected: boolean }) {
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    let animation: Animation | null = null;
    return onDeviceActivity(name, () => {
      animation?.cancel();
      animation = ref.current?.animate(
        [
          { opacity: 1, transform: "scale(1.35)" },
          { opacity: 0, transform: "scale(1)" },
        ],
        { duration: 500, easing: "ease-out" },
      ) ?? null;
    });
  }, [name]);
  return (
    <span className="relative flex size-[14px] shrink-0 items-center justify-center">
      <span className={cn("size-[7px] rounded-full", connected ? "bg-midi/45" : "bg-line-strong")} />
      <span ref={ref} className="absolute size-[9px] rounded-full bg-midi opacity-0 shadow-[0_0_8px_var(--midi)]" />
    </span>
  );
}

function DeviceItem({ port }: { port: PortStatus }) {
  const config = useApp((state) => state.config);
  const setPanel = useApp((state) => state.setPanel);
  const label = deviceLabel(port.name, config);
  return (
    <Tooltip
      side="right"
      content={
        port.state === "error"
          ? `${port.name} — ${port.error || "could not be opened"}`
          : port.state === "disabled"
            ? `${port.name} — disabled`
            : port.name !== label
              ? port.name
              : null
      }
    >
      <button
        type="button"
        onClick={() => setPanel("devices")}
        className="flex h-[26px] w-full items-center gap-2 rounded-md pr-1 pl-2 text-left text-fg-muted hover:bg-hover hover:text-fg"
      >
        {port.state === "error" ? (
          <AlertTriangle size={13} className="w-[14px] shrink-0 text-warn" />
        ) : (
          <DeviceLed name={port.name} connected={port.state === "connected"} />
        )}
        <span className={cn("min-w-0 flex-1 truncate", port.state === "disabled" && "line-through opacity-60")}>{label}</span>
      </button>
    </Tooltip>
  );
}

export function Sidebar({ onEditProfile }: { onEditProfile: (profile: Profile) => void }) {
  const profiles = useApp((state) => state.config.profiles);
  const ports = useApp((state) => state.ports);
  const addProfile = useApp((state) => state.addProfile);
  const importProfiles = useApp((state) => state.importProfiles);
  const setPanel = useApp((state) => state.setPanel);
  const toast = useApp((state) => state.toast);

  const doImport = async () => {
    const backend = await api();
    try {
      const imported = await backend.importProfiles();
      if (!imported) return;
      if (imported.length === 0) {
        toast("That file does not contain any profiles", { kind: "warning" });
        return;
      }
      importProfiles(imported);
      toast(`Imported ${imported.length === 1 ? `“${imported[0].name}”` : `${imported.length} profiles`}`, { kind: "success" });
    } catch (error) {
      toast(`Import failed: ${String(error)}`, { kind: "error" });
    }
  };

  return (
    <nav aria-label="Profiles and devices" className="flex w-[236px] shrink-0 flex-col border-r bg-surface">
      <div className="flex h-[34px] shrink-0 items-center pr-1.5 pl-3">
        <span className="flex-1 text-xs font-semibold tracking-wider text-fg-faint uppercase">Profiles</span>
        <Menu>
          <MenuTrigger asChild>
            <IconButton label="Import profiles or examples" size="sm">
              <Download size={13} />
            </IconButton>
          </MenuTrigger>
          <MenuContent align="start" className="max-h-[420px] w-[300px] overflow-y-auto">
            <MenuItem icon={<FolderOpen size={14} />} onSelect={doImport}>
              Import from a file…
            </MenuItem>
            <MenuSeparator />
            <MenuLabel>Examples to pick apart</MenuLabel>
            {EXAMPLES.map((example) => (
              <MenuItem key={example.file} className="h-auto py-1" onSelect={() => importExample(example)}>
                <div className="min-w-0">
                  <div className="truncate">{example.title}</div>
                  <div className="truncate text-sm text-fg-faint">{example.detail}</div>
                </div>
              </MenuItem>
            ))}
          </MenuContent>
        </Menu>
        <IconButton
          label="New profile"
          size="sm"
          onClick={() => addProfile(`Profile ${profiles.length + 1}`)}
        >
          <Plus size={14} />
        </IconButton>
      </div>
      <div className="flex min-h-0 flex-1 flex-col gap-[2px] overflow-y-auto px-1.5 pb-2">
        {profiles.map((profile) => (
          <ProfileItem key={profile.id} profile={profile} onEdit={onEditProfile} />
        ))}
      </div>

      <div className="shrink-0 border-t">
        <div className="flex h-[34px] items-center pr-1.5 pl-3">
          <span className="flex-1 text-xs font-semibold tracking-wider text-fg-faint uppercase">MIDI devices</span>
          <IconButton label="Manage devices" size="sm" onClick={() => setPanel("devices")}>
            <Settings2 size={13} />
          </IconButton>
        </div>
        <div className="flex max-h-[168px] flex-col gap-[1px] overflow-y-auto px-1.5 pb-2">
          {ports.length === 0 ? (
            <div className="px-2 pb-1 text-sm leading-snug text-fg-faint">
              No MIDI devices found. Plug one in — it is picked up automatically.
            </div>
          ) : (
            ports.map((port) => <DeviceItem key={port.name} port={port} />)
          )}
        </div>
      </div>
    </nav>
  );
}
