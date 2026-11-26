import { ClipboardCopy, Copy, FolderInput, FolderPlus, Power, PowerOff, Trash2, X } from "lucide-react";

import type { Mapping } from "@/bindings/Mapping";
import type { Profile } from "@/bindings/Profile";
import { Button, IconButton } from "@/components/ui/button";
import { Kbd } from "@/components/ui/misc";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/overlays";
import { mappingBrief, sourceTitle } from "@/lib/describe";
import { mappingsToText, useApp, useOctave } from "@/lib/store";
import { plural } from "@/lib/utils";

/** The picked mappings, in the order they are listed. */
export function pickedMappings(ids: readonly string[]): Mapping[] {
  const pick = new Set(ids);
  return useApp.getState().config.profiles.flatMap((profile) => profile.mappings.filter((m) => pick.has(m.id)));
}

/** Actions on several mappings at once, each with the message (and Undo) that belongs to it. */
export const bulk = {
  remove(ids: readonly string[]) {
    const state = useApp.getState();
    state.removeMappings([...ids]);
    state.toast(`${plural(ids.length, "mapping")} deleted`, { action: { label: "Undo", run: state.undo } });
  },
  setEnabled(ids: readonly string[], enabled: boolean) {
    useApp.getState().setMappingsEnabled([...ids], enabled);
  },
  duplicate(ids: readonly string[]) {
    useApp.getState().cloneMappings([...ids]);
  },
  copy(ids: readonly string[]) {
    const state = useApp.getState();
    void navigator.clipboard
      .writeText(mappingsToText(pickedMappings(ids)))
      .then(() => state.toast(`${plural(ids.length, "mapping")} copied. Paste with Ctrl+V — here, in another profile, or as text anywhere.`))
      .catch(() => state.toast("The clipboard is not available here. Press Ctrl+C instead.", { kind: "warning" }));
  },
  moveTo(ids: readonly string[], profile: Profile) {
    const state = useApp.getState();
    state.moveMappingsToProfile([...ids], profile.id);
    state.toast(`${plural(ids.length, "mapping")} moved to “${profile.name}”`, { action: { label: "Undo", run: state.undo } });
  },
  copyTo(ids: readonly string[], profile: Profile) {
    const state = useApp.getState();
    state.copyMappingsToProfile([...ids], profile.id);
    state.toast(`${plural(ids.length, "mapping")} copied to “${profile.name}”`, { action: { label: "Undo", run: state.undo } });
  },
};

/** Takes the place of the editor while several mappings are picked. */
export function BulkPanel({ ids }: { ids: string[] }) {
  const config = useApp((state) => state.config);
  const viewProfileId = useApp((state) => state.viewProfileId);
  const clearMulti = useApp((state) => state.clearMulti);
  const octave = useOctave();
  const mappings = pickedMappings(ids);
  const others = config.profiles.filter((p) => p.id !== viewProfileId);
  const anyOff = mappings.some((m) => !m.enabled);
  const anyOn = mappings.some((m) => m.enabled);

  return (
    <aside
      aria-label="Selected mappings"
      className="z-20 flex h-full w-[452px] max-w-full shrink-0 animate-slide-in flex-col border-l bg-surface max-[1180px]:absolute max-[1180px]:top-0 max-[1180px]:right-0 max-[1180px]:bottom-0 max-[1180px]:shadow-pop"
    >
      <header className="flex h-[46px] shrink-0 items-center gap-2 border-b px-4">
        <span className="min-w-0 flex-1 text-lg font-semibold">{plural(mappings.length, "mapping")} selected</span>
        <IconButton label="Clear the selection" shortcut="Esc" onClick={clearMulti}>
          <X size={16} />
        </IconButton>
      </header>
      <div className="min-h-0 flex-1 overflow-y-auto p-4">
        <div className="grid grid-cols-2 gap-2">
          <Button icon={<Power size={14} />} disabled={!anyOff} onClick={() => bulk.setEnabled(ids, true)}>
            Enable
          </Button>
          <Button icon={<PowerOff size={14} />} disabled={!anyOn} onClick={() => bulk.setEnabled(ids, false)}>
            Disable
          </Button>
          <Button icon={<Copy size={14} />} onClick={() => bulk.duplicate(ids)}>
            Duplicate
          </Button>
          <Button icon={<ClipboardCopy size={14} />} onClick={() => bulk.copy(ids)}>
            Copy
          </Button>
          {others.length > 0 && (
            <>
              <Menu>
                <MenuTrigger asChild>
                  <Button icon={<FolderInput size={14} />}>Move to profile…</Button>
                </MenuTrigger>
                <MenuContent align="start">
                  {others.map((p) => (
                    <MenuItem key={p.id} onSelect={() => bulk.moveTo(ids, p)}>
                      {p.name}
                    </MenuItem>
                  ))}
                </MenuContent>
              </Menu>
              <Menu>
                <MenuTrigger asChild>
                  <Button icon={<FolderPlus size={14} />}>Copy to profile…</Button>
                </MenuTrigger>
                <MenuContent align="start">
                  {others.map((p) => (
                    <MenuItem key={p.id} onSelect={() => bulk.copyTo(ids, p)}>
                      {p.name}
                    </MenuItem>
                  ))}
                </MenuContent>
              </Menu>
            </>
          )}
          <Button variant="danger" className="col-span-2" icon={<Trash2 size={14} />} onClick={() => bulk.remove(ids)}>
            Delete {plural(mappings.length, "mapping")}
          </Button>
        </div>

        <div className="mt-5 flex flex-col gap-1">
          {mappings.slice(0, 12).map((mapping) => (
            <div key={mapping.id} className="flex items-center gap-2 text-sm">
              <span className="w-[86px] shrink-0 truncate font-semibold text-midi">{sourceTitle(mapping.source.event, octave)}</span>
              <span className="min-w-0 truncate text-fg-muted">{mappingBrief(mapping, config)}</span>
            </div>
          ))}
          {mappings.length > 12 && <div className="text-sm text-fg-faint">…and {mappings.length - 12} more</div>}
        </div>
      </div>
      <div className="border-t px-4 py-3 text-sm leading-relaxed text-fg-faint">
        <Kbd>Ctrl</Kbd> click adds or removes a mapping, <Kbd>Shift</Kbd> click picks a range, <Kbd>Ctrl+A</Kbd> picks all.
      </div>
    </aside>
  );
}
