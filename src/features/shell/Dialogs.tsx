import { AlertTriangle, FolderOpen, Plus, RefreshCw, Trash2 } from "lucide-react";
import type { ReactNode } from "react";

import type { Profile } from "@/bindings/Profile";
import type { Settings } from "@/bindings/Settings";
import { Button, IconButton } from "@/components/ui/button";
import { NumberInput, Segmented, Select, Switch, TextInput } from "@/components/ui/inputs";
import { Badge, Option } from "@/components/ui/misc";
import { Dialog } from "@/components/ui/overlays";
import { api, useApp } from "@/lib/store";
import { cn } from "@/lib/utils";

function Group({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="mb-6 last:mb-0">
      <h3 className="mb-3 text-xs font-semibold tracking-wider text-fg-faint uppercase">{title}</h3>
      <div className="flex flex-col gap-3.5">{children}</div>
    </section>
  );
}

// ---------------------------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------------------------

export function SettingsDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const settings = useApp((state) => state.config.settings);
  const info = useApp((state) => state.info);
  const permissions = useApp((state) => state.permissions);
  const backendError = useApp((state) => state.backendError);
  const backendName = useApp((state) => state.engine.backend);
  const updateSettings = useApp((state) => state.updateSettings);
  const set = <K extends keyof Settings>(key: K, value: Settings[K], coalesce?: string) =>
    updateSettings((s) => ({ ...s, [key]: value }), coalesce);

  return (
    <Dialog open={open} onOpenChange={onOpenChange} title="Settings" width={600}>
      {(backendError || !permissions.can_inject) && (
        <div className="mb-5 flex gap-2.5 rounded-lg bg-warn-soft p-3 text-warn">
          <AlertTriangle size={16} className="mt-[2px] shrink-0" />
          <div className="min-w-0 flex-1">
            <div className="font-semibold">Keys cannot be sent yet</div>
            <div className="selectable mt-0.5 text-sm whitespace-pre-wrap">{backendError || permissions.hint}</div>
            <Button size="sm" className="mt-2" onClick={async () => (await api()).requestPermissions()}>
              Open system permission prompt
            </Button>
          </div>
        </div>
      )}

      <Group title="General">
        <Option title="Appearance">
          <Segmented
            value={settings.theme}
            options={[
              { value: "system", label: "System" },
              { value: "dark", label: "Dark" },
              { value: "light", label: "Light" },
            ]}
            onChange={(theme) => set("theme", theme)}
          />
        </Option>
        <Option title="Launch at login" description="Start MIDI 2 Keyboard when you sign in.">
          <Switch aria-label="Launch at login" checked={settings.launch_at_login} onChange={(v) => set("launch_at_login", v)} />
        </Option>
        <Option title="Start in the tray" description="Open without showing this window.">
          <Switch aria-label="Start in the tray" checked={settings.start_minimized} onChange={(v) => set("start_minimized", v)} />
        </Option>
        <Option title="Keep running when the window is closed" description="Mappings keep working; reopen from the tray icon.">
          <Switch aria-label="Keep running in the tray" checked={settings.close_to_tray} onChange={(v) => set("close_to_tray", v)} />
        </Option>
      </Group>

      <Group title="Behavior">
        <Option
          title="Hold back output while this window is in front"
          description="Stops your mappings from typing into (or closing) this editor while you set things up. The monitor still shows what would have been sent."
        >
          <Switch
            aria-label="Hold back output while this window is in front"
            checked={settings.suspend_while_focused}
            onChange={(v) => set("suspend_while_focused", v)}
          />
        </Option>
        <Option
          title="Switch profiles automatically"
          description={
            permissions.can_detect_foreground
              ? "Activate a profile while one of its applications is in front."
              : "Unavailable: the foreground application cannot be detected on this system."
          }
        >
          <Switch
            aria-label="Switch profiles automatically"
            disabled={!permissions.can_detect_foreground}
            checked={settings.auto_switch_profiles}
            onChange={(v) => set("auto_switch_profiles", v)}
          />
        </Option>
        <Option title="Connect new MIDI devices automatically" description="Off: only devices you enabled under MIDI devices are opened.">
          <Switch aria-label="Connect new devices" checked={settings.connect_new_devices} onChange={(v) => set("connect_new_devices", v)} />
        </Option>
      </Group>

      <Group title="Timing">
        <Option title="Key tap length" description="How long a tapped key stays down. Some games ignore very short taps — try 40–60 ms there.">
          <NumberInput aria-label="Key tap length" suffix="ms" className="w-[88px]" min={1} max={5000} value={settings.tap_ms} onChange={(v) => set("tap_ms", v, "tap")} />
        </Option>
        <Option title="Delay between keys of a shortcut" description="Pause between modifier and key events. Raise it if an app misses shortcuts (remote desktops, VMs).">
          <NumberInput aria-label="Delay between keys" suffix="ms" className="w-[88px]" min={0} max={1000} value={settings.key_stagger_ms} onChange={(v) => set("key_stagger_ms", v, "stagger")} />
        </Option>
        <Option title="Held-key repeat" description="For Hold mappings with auto-repeat: delay before repeating, then the interval between repeats.">
          <div className="flex items-center gap-1.5">
            <NumberInput aria-label="Repeat delay" suffix="ms" className="w-[88px]" min={50} max={10000} value={settings.repeat_delay_ms} onChange={(v) => set("repeat_delay_ms", v, "rdelay")} />
            <NumberInput aria-label="Repeat interval" suffix="ms" className="w-[88px]" min={5} max={10000} value={settings.repeat_interval_ms} onChange={(v) => set("repeat_interval_ms", v, "rint")} />
          </div>
        </Option>
      </Group>

      <Group title="Output">
        {info.platform === "windows" && (
          <Option
            title="How keys are sent"
            description="Automatic suits almost everything. Scan codes are what some DirectInput / raw-input games need; virtual keys ignore the physical layout."
          >
            <Select
              aria-label="Windows key mode"
              className="w-[170px]"
              value={settings.injector.windows_key_mode}
              options={[
                { value: "auto", label: "Automatic" },
                { value: "scan_code", label: "Scan codes" },
                { value: "virtual_key", label: "Virtual keys" },
              ]}
              onChange={(mode) => set("injector", { ...settings.injector, windows_key_mode: mode })}
            />
          </Option>
        )}
        {info.platform === "linux" && (
          <Option title="Input backend" description="uinput works on Wayland and X11 but needs access to /dev/uinput; X11 needs no setup.">
            <Select
              aria-label="Linux backend"
              className="w-[170px]"
              value={settings.injector.linux_backend}
              options={[
                { value: "auto", label: "Automatic" },
                { value: "uinput", label: "uinput" },
                { value: "x11", label: "X11 (XTest)" },
              ]}
              onChange={(backend) => set("injector", { ...settings.injector, linux_backend: backend })}
            />
          </Option>
        )}
        <Option title="Middle C is" description="Which octave number note 60 gets. Hardware and DAWs disagree; pick what your gear prints.">
          <Segmented
            value={String(settings.middle_c_octave)}
            options={[
              { value: "3", label: "C3" },
              { value: "4", label: "C4" },
              { value: "5", label: "C5" },
            ]}
            onChange={(value) => set("middle_c_octave", Number(value))}
          />
        </Option>
      </Group>

      <Group title="About">
        <div className="flex items-center gap-3">
          <div className="min-w-0 flex-1">
            <div>
              MIDI 2 Keyboard <span className="text-fg-muted">{info.version}</span>
              {backendName && <span className="text-fg-faint"> · output via {backendName}</span>}
            </div>
            <div className="selectable truncate text-sm text-fg-faint">{info.config_path}</div>
          </div>
          <Button icon={<FolderOpen size={14} />} onClick={async () => (await api()).revealConfig()}>
            Show config file
          </Button>
        </div>
      </Group>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------------------------

export function DevicesDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const ports = useApp((state) => state.ports);
  const devices = useApp((state) => state.config.devices);
  const connectNew = useApp((state) => state.config.settings.connect_new_devices);
  const updateDevice = useApp((state) => state.updateDevice);
  const forgetDevice = useApp((state) => state.forgetDevice);
  const platformName = useApp((state) => state.info.platform);

  const names = Array.from(new Set([...ports.map((p) => p.name), ...devices.map((d) => d.name)]));

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="MIDI devices"
      description="Devices are picked up as soon as they are plugged in. Give them a friendlier name, or switch off the ones this app should leave alone."
      width={620}
      footer={
        <Button icon={<RefreshCw size={13} />} onClick={async () => (await api()).rescanDevices()}>
          Rescan now
        </Button>
      }
    >
      {names.length === 0 && (
        <div className="py-8 text-center text-fg-faint">No MIDI devices found. Plug one in and it appears here.</div>
      )}
      <div className="flex flex-col gap-2">
        {names.map((name) => {
          const port = ports.find((p) => p.name === name);
          const device = devices.find((d) => d.name === name);
          const enabled = device ? device.enabled : connectNew;
          return (
            <div key={name} className="rounded-lg border bg-app p-3">
              <div className="flex items-center gap-3">
                <span
                  className={cn(
                    "size-[9px] shrink-0 rounded-full",
                    port?.state === "connected" ? "bg-midi shadow-[0_0_6px_var(--midi)]" : port?.state === "error" ? "bg-warn" : "bg-line-strong",
                  )}
                />
                <div className="min-w-0 flex-1">
                  <div className="selectable truncate font-medium">{name}</div>
                  <div className="text-sm text-fg-faint">
                    {!port
                      ? "Not connected"
                      : port.state === "connected"
                        ? port.is_virtual
                          ? "Virtual port — other apps can send MIDI here"
                          : "Connected"
                        : port.state === "disabled"
                          ? "Present, switched off"
                          : "Could not be opened"}
                  </div>
                </div>
                <TextInput
                  aria-label="Friendly name"
                  className="w-[170px]"
                  placeholder="Friendly name"
                  value={device?.alias ?? ""}
                  onChange={(alias) => updateDevice(name, (d) => ({ ...d, alias }), "alias")}
                />
                <Switch
                  tone="midi"
                  aria-label={`Use ${name}`}
                  checked={enabled}
                  onChange={(value) => updateDevice(name, (d) => ({ ...d, enabled: value }))}
                />
                {!port && (
                  <IconButton label="Forget this device" size="sm" tone="danger" onClick={() => forgetDevice(name)}>
                    <Trash2 size={13} />
                  </IconButton>
                )}
              </div>
              {port?.state === "error" && (
                <div className="mt-2 flex gap-2 rounded-md bg-warn-soft px-2.5 py-1.5 text-sm text-warn">
                  <AlertTriangle size={14} className="mt-[1px] shrink-0" />
                  <span className="selectable">
                    {port.error || "The device could not be opened."}
                    {platformName === "windows" &&
                      " On Windows most MIDI drivers allow only one program at a time: close the other program using it (a DAW, the vendor's editor) or route through a virtual port such as loopMIDI."}
                  </span>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------------------------
// Profile settings
// ---------------------------------------------------------------------------------------------

export function ProfileDialog({ profile, onClose }: { profile: Profile | null; onClose: () => void }) {
  const live = useApp((state) => state.config.profiles.find((p) => p.id === profile?.id) ?? null);
  const updateProfile = useApp((state) => state.updateProfile);
  const foreground = useApp((state) => state.engine.foreground);
  const canDetect = useApp((state) => state.permissions.can_detect_foreground);
  const autoSwitch = useApp((state) => state.config.settings.auto_switch_profiles);
  if (!live) return null;
  const update = (change: (p: Profile) => Profile, key?: string) => updateProfile(live.id, change, key);

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()} title="Profile settings" width={540}>
      <Group title="Profile">
        <Option title="Name">
          <TextInput aria-label="Profile name" className="w-[260px]" value={live.name} onChange={(name) => update((p) => ({ ...p, name }), "name")} />
        </Option>
        <Option
          title="Always on"
          description="Its mappings work no matter which profile is selected. Good for global controls — a pad that switches profiles, a panic button, media keys."
        >
          <Switch aria-label="Always on" checked={live.always_on} onChange={(always_on) => update((p) => ({ ...p, always_on }))} />
        </Option>
      </Group>

      {!live.always_on && (
        <Group title="Activate automatically for these apps">
          {!canDetect && (
            <div className="rounded-md bg-warn-soft px-2.5 py-1.5 text-sm text-warn">
              The foreground application cannot be detected on this system, so automatic switching is unavailable.
            </div>
          )}
          {canDetect && !autoSwitch && live.apps.length > 0 && (
            <div className="rounded-md bg-warn-soft px-2.5 py-1.5 text-sm text-warn">
              Automatic profile switching is turned off in Settings.
            </div>
          )}
          {live.apps.length === 0 && (
            <div className="text-sm text-fg-faint">
              Add an application and this profile takes over whenever that app is in front — and steps
              back when you leave it.
            </div>
          )}
          {live.apps.map((app, index) => (
            <div key={index} className="flex items-center gap-2">
              <TextInput
                aria-label="Process name"
                placeholder="Process, e.g. Photoshop.exe"
                value={app.process}
                onChange={(process) =>
                  update((p) => ({ ...p, apps: p.apps.map((a, i) => (i === index ? { ...a, process } : a)) }), `proc${index}`)
                }
              />
              <TextInput
                aria-label="Window title"
                placeholder="Title contains… (optional)"
                value={app.title}
                onChange={(title) =>
                  update((p) => ({ ...p, apps: p.apps.map((a, i) => (i === index ? { ...a, title } : a)) }), `title${index}`)
                }
              />
              <IconButton
                label="Remove"
                size="sm"
                tone="danger"
                onClick={() => update((p) => ({ ...p, apps: p.apps.filter((_, i) => i !== index) }))}
              >
                <Trash2 size={13} />
              </IconButton>
            </div>
          ))}
          <div className="flex items-center gap-2">
            <Button
              size="sm"
              icon={<Plus size={13} />}
              onClick={() => update((p) => ({ ...p, apps: [...p.apps, { process: "", title: "" }] }))}
            >
              Add application
            </Button>
            {foreground?.process && (
              <Badge title="The application most recently in front">last seen: {foreground.process}</Badge>
            )}
          </div>
          <div className="text-sm text-fg-faint">
            Names are matched without regard to case; <code className="font-mono">*</code> and{" "}
            <code className="font-mono">?</code> work as wildcards. When several profiles match, the first one in
            the list wins.
          </div>
        </Group>
      )}
    </Dialog>
  );
}
