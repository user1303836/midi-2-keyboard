import {
  ArrowDown,
  ArrowRight,
  ArrowUp,
  ChevronDown,
  ClipboardCopy,
  Copy,
  CopyPlus,
  Filter,
  FolderInput,
  Gauge,
  FolderPlus,
  GripVertical,
  Lightbulb,
  LocateFixed,
  MoreHorizontal,
  Pencil,
  Piano,
  Plus,
  Power,
  Search,
  SlidersHorizontal,
  Timer,
  Trash2,
  X,
} from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { Config } from "@/bindings/Config";
import type { Mapping } from "@/bindings/Mapping";
import type { Profile } from "@/bindings/Profile";
import { Button } from "@/components/ui/button";
import { Switch } from "@/components/ui/inputs";
import { Badge, Kbd } from "@/components/ui/misc";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuSub,
  ContextMenuSubContent,
  ContextMenuSubTrigger,
  ContextMenuTrigger,
  Menu,
  MenuContent,
  MenuItem,
  MenuTrigger,
  Tooltip,
} from "@/components/ui/overlays";
import { conditionText, mappingSentence, rateLimitText, sourceNickname, sourceQualifiers, sourceTitle } from "@/lib/describe";
import { findConflicts, listensTo, type Conflict } from "@/lib/conflicts";
import type { MidiLog } from "@/bindings/MidiLog";
import { EXAMPLES, importExample } from "@/lib/examples";
import { sourceForMessage } from "@/lib/model";
import { onMappingActivity, onMidi, type ActivityKind } from "@/lib/monitor";
import { api, mappingsToText, useApp, useOctave } from "@/lib/store";
import { cn, plural } from "@/lib/utils";

import { mapMessage } from "../monitor/Monitor";
import { bulk, pickedMappings } from "./BulkPanel";
import { NoteRunDialog } from "./NoteRunDialog";
import { ZonesDialog, type ZoneKind } from "./ZonesDialog";
import { BehaviorSummary, SourceIcon } from "./tokens";

const FLASH: Keyframe[] = [
  { opacity: 1, transform: "scale(1.35)" },
  { opacity: 0, transform: "scale(1)" },
];

/** The little light on each row: teal when its source matched, amber when its action went out. */
function ActivityLed({ mappingId }: { mappingId: string }) {
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    let shown: ActivityKind | null = null;
    let animation: Animation | null = null;
    return onMappingActivity(mappingId, (kind: ActivityKind) => {
      const element = ref.current;
      if (!element) return;
      if (kind !== shown) {
        shown = kind;
        const color =
          kind === "sent" ? "var(--key)" : kind === "failed" ? "var(--danger)" : kind === "matched" ? "var(--midi)" : "var(--fg-faint)";
        element.style.background = color;
        element.style.boxShadow = `0 0 8px ${color}`;
      }
      // The Web Animations API restarts cleanly without forcing a layout (unlike toggling a class).
      animation?.cancel();
      animation = element.animate(FLASH, { duration: 500, easing: "ease-out" });
    });
  }, [mappingId]);
  return (
    <span className="relative flex size-[8px] items-center justify-center">
      <span className="absolute size-[6px] rounded-full bg-line-strong" />
      <span ref={ref} className="absolute size-[8px] rounded-full opacity-0" />
    </span>
  );
}

/** The ids of the rows as they are shown right now (a search may hide some). */
function shownOrder(): string[] {
  return Array.from(document.querySelectorAll<HTMLElement>('[role="row"][data-mapping-id]'), (row) => row.dataset.mappingId!);
}

/**
 * One mapping. Memoized on purpose: with hundreds of mappings and an engine that reports status
 * many times a second, a row must only re-render when *its* mapping (or its own flags) changed.
 * That is why every callback is a stable function taking the id / index, and why the row does
 * not subscribe to the whole config.
 */
const MappingRow = memo(function MappingRow({
  mapping,
  index,
  selected,
  engaged,
  dragging,
  dropBefore,
  onDragStart,
  onDragOver,
  onDrop,
  onDragEnd,
  draggable,
  conflicts,
  menuTarget,
  picked,
}: {
  mapping: Mapping;
  index: number;
  selected: boolean;
  /** Part of a selection of several mappings (Ctrl / Shift click). */
  picked: boolean;
  /** The right-click menu is open for this row. */
  menuTarget: boolean;
  engaged: boolean;
  conflicts: Conflict[] | undefined;
  dragging: boolean;
  dropBefore: boolean;
  onDragStart: (id: string) => void;
  onDragOver: (index: number, event: React.DragEvent) => void;
  onDrop: () => void;
  onDragEnd: () => void;
  draggable: boolean;
}) {
  const selectMapping = useApp((state) => state.selectMapping);
  const updateMapping = useApp((state) => state.updateMapping);
  // Only the device list (for friendly names), not the whole config.
  const devices = useApp((state) => state.config.devices);
  const octave = useOctave();

  const event = mapping.source.event;
  const qualifiers = sourceQualifiers(mapping.source, { devices } as Config);
  const nickname = sourceNickname(event);

  return (
    <div
      role="row"
      data-mapping-id={mapping.id}
      aria-selected={selected}
      tabIndex={0}
      draggable={draggable}
      onDragStart={(e) => {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", mapping.id);
        // Lets a profile in the sidebar take the mapping (dropping it there moves it).
        e.dataTransfer.setData("application/x-m2k-mapping", mapping.id);
        onDragStart(mapping.id);
      }}
      onDragOver={(e) => onDragOver(index, e)}
      onDrop={onDrop}
      onDragEnd={onDragEnd}
      onMouseDown={(e) => {
        // Shift+click picks a range; it must not select the text in between.
        if (e.shiftKey) e.preventDefault();
      }}
      onClick={(e) => {
        const state = useApp.getState();
        if (e.shiftKey) state.selectRange(mapping.id, shownOrder());
        else if (e.ctrlKey || e.metaKey) state.toggleSelect(mapping.id);
        else selectMapping(selected && !picked ? null : mapping.id);
      }}
      onKeyDown={(e) => {
        if (e.target !== e.currentTarget) return;
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          selectMapping(mapping.id);
        }
      }}
      onMouseEnter={(e) => {
        // The full sentence is only worked out when somebody actually hovers.
        e.currentTarget.title = mappingSentence(mapping, useApp.getState().config, octave);
      }}
      className={cn(
        "group relative flex min-h-[46px] cursor-pointer items-center gap-2.5 rounded-lg border px-2.5 py-1.5 outline-hidden transition-colors [contain-intrinsic-size:auto_46px] [content-visibility:auto]",
        selected ? "border-accent-line bg-accent-soft" : "border-transparent hover:bg-hover focus-visible:border-accent-line",
        menuTarget && !selected && "border-line-strong bg-hover",
        dragging && "opacity-40",
        dropBefore && "shadow-[0_-2px_0_0_var(--accent)]",
      )}
    >
      {draggable && (
        <GripVertical
          size={13}
          className="-mr-1.5 -ml-1 shrink-0 cursor-grab text-fg-faint opacity-0 transition-opacity group-hover:opacity-100"
        />
      )}
      <ActivityLed mappingId={mapping.id} />

      {/* When */}
      <div className={cn("flex w-[150px] shrink-0 items-center gap-2 @min-[700px]:w-[236px] @min-[900px]:w-[300px] @min-[1100px]:w-[380px]", !mapping.enabled && "opacity-45")}>
        <span className="flex h-[26px] max-w-full min-w-0 shrink-0 items-center gap-1.5 rounded-md border border-midi-line bg-midi-soft px-2 font-semibold text-midi">
          <SourceIcon event={event} size={14} className="shrink-0" />
          <span className="truncate">{sourceTitle(event, octave)}</span>
        </span>
        <span className="min-w-0 truncate text-sm text-fg-muted">
          {[nickname, ...qualifiers].filter(Boolean).join(" · ")}
        </span>
      </div>

      <ArrowRight size={14} className="shrink-0 text-fg-faint" />

      {/* Do */}
      <div className={cn("flex min-w-0 flex-1 flex-col justify-center", !mapping.enabled && "opacity-45")}>
        {mapping.name && <div className="truncate text-sm leading-tight text-fg-muted">{mapping.name}</div>}
        <div className="flex min-w-0 items-center gap-1.5 overflow-hidden">
          {mapping.behavior.type === "change" && <span className="shrink-0 text-sm text-fg-muted">on change</span>}
          <BehaviorSummary behavior={mapping.behavior} event={mapping.source.event} />
        </div>
      </div>

      {/* Meta */}
      <div className="flex shrink-0 items-center gap-1.5">
        {engaged && <Badge tone="key">ON</Badge>}
        {conflicts && conflicts.length > 0 && (
          <Tooltip
            content={
              <>
                The same control also triggers{" "}
                {conflicts
                  .map((c) => `“${c.mapping.name || sourceTitle(c.mapping.source.event, octave)}”${c.profile ? ` in ${c.profile}` : ""}`)
                  .join(", ")}
                . Both will fire. Give them different conditions if that is not what you want.
              </>
            }
          >
            <span>
              <Badge tone="warn">
                <CopyPlus size={11} />
                shared
              </Badge>
            </span>
          </Tooltip>
        )}
        {mapping.feedback && (
          <Badge tone="midi" title="Sends MIDI back to the controller to show its state (a light)">
            <Lightbulb size={11} />
          </Badge>
        )}
        {mapping.rate_limit && (
          <Badge tone="accent" title={`Rate limit: ${rateLimitText(mapping.rate_limit)}`}>
            <Timer size={11} />
            <span className="hidden @min-[700px]:inline">{rateLimitText(mapping.rate_limit)}</span>
          </Badge>
        )}
        {mapping.conditions.length > 0 && (
          <Badge tone="neutral" title={mapping.conditions.map((c) => conditionText(c, null, octave)).join("\n")}>
            <Filter size={11} />
            {/* Always shown: two mappings on the same control often differ only by their condition. */}
            <span className="max-w-[104px] truncate @min-[700px]:max-w-[200px]">
              {mapping.conditions.length === 1 ? conditionText(mapping.conditions[0], null, octave) : `${mapping.conditions.length} conditions`}
            </span>
          </Badge>
        )}
      </div>

      <div className="flex shrink-0 items-center gap-1" onClick={(e) => e.stopPropagation()}>
        <span className="flex" title={mapping.enabled ? "Enabled" : "Disabled"}>
          <Switch
            tone="midi"
            aria-label="Enabled"
            checked={mapping.enabled}
            onChange={(enabled) => updateMapping(mapping.id, (m) => ({ ...m, enabled }))}
          />
        </span>
        {/* Opens the list's one right-click menu, anchored under the button. */}
        <button
          type="button"
          aria-label="More"
          aria-haspopup="menu"
          title="More"
          className={cn(
            "inline-flex size-[24px] items-center justify-center rounded-md text-fg-muted opacity-0 outline-hidden transition-opacity group-hover:opacity-100 hover:bg-hover hover:text-fg focus-visible:opacity-100 focus-visible:ring-2 focus-visible:ring-accent",
            menuTarget && "opacity-100",
          )}
          onClick={(e) => {
            const box = e.currentTarget.getBoundingClientRect();
            e.currentTarget.dispatchEvent(
              new MouseEvent("contextmenu", { bubbles: true, cancelable: true, clientX: box.left, clientY: box.bottom + 4 }),
            );
          }}
        >
          <MoreHorizontal size={15} />
        </button>
      </div>
    </div>
  );
});

function matchesSearch(mapping: Mapping, query: string, sentence: string): boolean {
  if (!query) return true;
  const haystack = `${mapping.name} ${sentence}`.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .every((word) => haystack.includes(word));
}

export function MappingList({ profile }: { profile: Profile }) {
  const selectedId = useApp((state) => state.selectedMappingId);
  const multi = useApp((state) => state.multi);
  const engaged = useApp((state) => state.engine.engaged_mappings);
  const search = useApp((state) => state.search);
  const setSearch = useApp((state) => state.setSearch);
  const startWizard = useApp((state) => state.startWizard);
  const moveMapping = useApp((state) => state.moveMapping);
  const config = useApp((state) => state.config);
  const octave = useOctave();
  const [runOpen, setRunOpen] = useState(false);
  const [zones, setZones] = useState<ZoneKind | null>(null);
  const [dragId, setDragId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
  // One right-click menu for the whole list (not one per row): which row it is for, if any.
  const [menu, setMenu] = useState<{ id: string | null; open: boolean }>({ id: null, open: false });
  const menuMapping = menu.id ? (profile.mappings.find((m) => m.id === menu.id) ?? null) : null;
  const otherProfiles = useMemo(() => config.profiles.filter((p) => p.id !== profile.id), [config.profiles, profile.id]);
  // Latest drag state for the stable row callbacks below.
  const drag = useRef({ dragId, dropIndex, mappings: profile.mappings });
  drag.current = { dragId, dropIndex, mappings: profile.mappings };

  const handleDragStart = useCallback((id: string) => setDragId(id), []);
  const handleDragOver = useCallback((index: number, e: React.DragEvent) => {
    if (!drag.current.dragId) return;
    e.preventDefault();
    const rect = e.currentTarget.getBoundingClientRect();
    const after = e.clientY > rect.top + rect.height / 2;
    setDropIndex(after ? index + 1 : index);
  }, []);
  const handleDragEnd = useCallback(() => {
    setDragId(null);
    setDropIndex(null);
  }, []);
  const handleDrop = useCallback(() => {
    const { dragId: id, dropIndex: target, mappings } = drag.current;
    if (id && target !== null) {
      const from = mappings.findIndex((m) => m.id === id);
      moveMapping(id, target > from ? target - 1 : target);
    }
    setDragId(null);
    setDropIndex(null);
  }, [moveMapping]);

  // Without a search every mapping is shown: no need to spell each one out to find that out.
  const visible = useMemo(
    () =>
      search
        ? profile.mappings.filter((mapping) => matchesSearch(mapping, search, mappingSentence(mapping, config, octave)))
        : profile.mappings,
    [profile.mappings, search, config, octave],
  );

  const conflicts = useMemo(() => findConflicts(config, profile), [config, profile]);

  const quickAdd = startWizard;
  const reorderable = search === "";

  // "Which mapping is this knob?": touch a control and the list jumps to its mapping, wherever
  // it lives. The engine holds its fire meanwhile, like during any other learn.
  const [locating, setLocating] = useState(false);
  useEffect(() => {
    if (!locating) return;
    void api().then((backend) => backend.setLearning(true));
    const off = onMidi((log) => {
      const message = log.message;
      if (message.type === "clock" || message.type === "active_sensing" || message.type === "note_off") return;
      if (message.type === "note_on" && message.velocity === 0) return;
      const state = useApp.getState();
      const found: { profileId: string; mappingId: string }[] = [];
      for (const p of state.config.profiles) {
        for (const m of p.mappings) {
          if (listensTo(m.source, log.device, message)) found.push({ profileId: p.id, mappingId: m.id });
        }
      }
      setLocating(false);
      const hit = found.find((f) => f.profileId === state.viewProfileId) ?? found[0];
      if (!hit) {
        state.toast("No mapping uses that control yet.", { action: { label: "Add one", run: state.startWizard } });
        return;
      }
      if (hit.profileId !== state.viewProfileId) state.viewProfile(hit.profileId);
      state.setSearch("");
      state.selectMapping(hit.mappingId);
      requestAnimationFrame(() =>
        document
          .querySelector<HTMLElement>(`[data-mapping-id="${CSS.escape(hit.mappingId)}"]`)
          ?.scrollIntoView({ block: "center" }),
      );
      if (found.length > 1) state.toast(`${found.length} mappings use this control — this is the first.`);
    });
    const cancel = (event: KeyboardEvent) => event.key === "Escape" && setLocating(false);
    window.addEventListener("keydown", cancel, true);
    return () => {
      off();
      window.removeEventListener("keydown", cancel, true);
      void api().then((backend) => backend.setLearning(false));
    };
  }, [locating]);

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex h-[46px] shrink-0 items-center gap-2 border-b px-4">
        <div className="relative w-[240px] min-w-[120px] shrink">
          <Search size={14} className="pointer-events-none absolute top-1/2 left-2 -translate-y-1/2 text-fg-faint" />
          <input
            aria-label="Search mappings"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setSearch("")}
            placeholder="Search mappings"
            className="h-[28px] w-full rounded-md border border-line-strong bg-app pr-7 pl-7 outline-hidden placeholder:text-fg-faint hover:border-fg-faint focus:border-accent"
          />
          {search && (
            <button
              type="button"
              aria-label="Clear search"
              onClick={() => setSearch("")}
              className="absolute top-1/2 right-1.5 flex size-[18px] -translate-y-1/2 items-center justify-center rounded text-fg-faint hover:text-fg"
            >
              <X size={12} />
            </button>
          )}
        </div>
        <Tooltip content={locating ? "Touch a control… (Esc to cancel)" : "Find a mapping by touching its control"}>
          <button
            type="button"
            aria-label="Find a mapping by touching its control"
            aria-pressed={locating}
            onClick={() => setLocating(!locating)}
            className={cn(
              "inline-flex h-[28px] shrink-0 items-center gap-1.5 rounded-md border px-2 outline-hidden transition-colors focus-visible:ring-2 focus-visible:ring-accent",
              locating
                ? "animate-listen border-midi bg-midi-soft text-midi [--ring-color:var(--midi-line)]"
                : "border-transparent text-fg-muted hover:bg-hover hover:text-fg",
            )}
          >
            <LocateFixed size={15} />
            {locating && <span className="text-sm whitespace-nowrap">Touch a control…</span>}
          </button>
        </Tooltip>
        <span className="shrink-0 text-sm whitespace-nowrap text-fg-faint max-[1000px]:hidden">
          {multi.length > 1
            ? `${multi.length} selected`
            : search
              ? `${visible.length} of ${profile.mappings.length}`
              : plural(profile.mappings.length, "mapping")}
        </span>
        <div className="flex-1" />
        <div className="flex shrink-0">
          <Button variant="primary" className="rounded-r-none" icon={<Plus size={15} />} onClick={quickAdd}>
            Add mapping
          </Button>
          <Menu>
            <MenuTrigger asChild>
              <Button
                variant="primary"
                aria-label="More ways to add mappings"
                className="rounded-l-none border-l border-accent-fg/25 px-1.5"
              >
                <ChevronDown size={14} />
              </Button>
            </MenuTrigger>
            <MenuContent className="w-[290px]">
              <MenuItem icon={<Plus size={14} />} shortcut="N" className="h-auto py-1.5" onSelect={quickAdd}>
                <div>
                  <div>Add one mapping</div>
                  <div className="text-sm text-fg-faint">Touch a control, press a key</div>
                </div>
              </MenuItem>
              <MenuItem icon={<Piano size={14} />} className="h-auto py-1.5" onSelect={() => setRunOpen(true)}>
                <div>
                  <div>Map a run of notes…</div>
                  <div className="text-sm text-fg-faint">A stretch of keys or pads to a row of computer keys</div>
                </div>
              </MenuItem>
              <MenuItem icon={<SlidersHorizontal size={14} />} className="h-auto py-1.5" onSelect={() => setZones("position")}>
                <div>
                  <div>Split a knob into zones…</div>
                  <div className="text-sm text-fg-faint">A knob or fader as a selector switch: a key per position</div>
                </div>
              </MenuItem>
              <MenuItem icon={<Gauge size={14} />} className="h-auto py-1.5" onSelect={() => setZones("velocity")}>
                <div>
                  <div>Split a pad by how hard it is hit…</div>
                  <div className="text-sm text-fg-faint">Soft and hard hits send different keys</div>
                </div>
              </MenuItem>
            </MenuContent>
          </Menu>
        </div>
      </div>
      <NoteRunDialog open={runOpen} onOpenChange={setRunOpen} />
      {/* One instance per kind, so each starts out fresh. */}
      <ZonesDialog kind="position" open={zones === "position"} onOpenChange={(open) => setZones(open ? "position" : null)} />
      <ZonesDialog kind="velocity" open={zones === "velocity"} onOpenChange={(open) => setZones(open ? "velocity" : null)} />

      {profile.mappings.length === 0 ? (
        <EmptyState onAdd={quickAdd} />
      ) : (
        <ContextMenu onOpenChange={(open) => setMenu((current) => ({ ...current, open }))}>
          <ContextMenuTrigger asChild>
        <div
          role="grid"
          aria-label="Mappings"
          aria-multiselectable
          className="@container min-h-0 flex-1 overflow-y-auto px-2 py-2"
          onContextMenu={(e) => {
            const row = (e.target as HTMLElement).closest<HTMLElement>("[data-mapping-id]");
            setMenu({ id: row?.dataset.mappingId ?? null, open: true });
          }}
          onDragOver={(e) => {
            if (dragId) e.preventDefault();
          }}
        >
          {visible.length === 0 && (
            <div className="p-8 text-center text-fg-faint">No mapping matches “{search}”.</div>
          )}
          {visible.map((mapping, index) => (
            <MappingRow
              key={mapping.id}
              mapping={mapping}
              index={index}
              selected={mapping.id === selectedId || multi.includes(mapping.id)}
              picked={multi.includes(mapping.id)}
              menuTarget={menu.open && menu.id === mapping.id}
              engaged={engaged.includes(mapping.id)}
              conflicts={conflicts.get(mapping.id)}
              draggable={reorderable}
              dragging={dragId === mapping.id}
              dropBefore={dropIndex === index && dragId !== null && dragId !== mapping.id}
              onDragStart={handleDragStart}
              onDragOver={handleDragOver}
              onDrop={handleDrop}
              onDragEnd={handleDragEnd}
            />
          ))}
          {dropIndex === visible.length && dragId && <div className="mx-2 h-[2px] rounded-full bg-accent" />}
        </div>
          </ContextMenuTrigger>
          <ContextMenuContent
            onCloseAutoFocus={(event) => {
              if (!menu.id) return;
              event.preventDefault();
              document.querySelector<HTMLElement>(`[data-mapping-id="${CSS.escape(menu.id)}"]`)?.focus();
            }}
          >
            {menuMapping && multi.length > 1 && multi.includes(menuMapping.id) ? (
              <BulkMenu ids={multi} otherProfiles={otherProfiles} />
            ) : menuMapping ? (
              <RowMenu mapping={menuMapping} otherProfiles={otherProfiles} />
            ) : (
              <>
                <ContextMenuItem icon={<Plus size={14} />} shortcut="N" onSelect={quickAdd}>
                  Add mapping
                </ContextMenuItem>
                <ContextMenuItem icon={<Piano size={14} />} onSelect={() => setRunOpen(true)}>
                  Map a run of notes…
                </ContextMenuItem>
                <ContextMenuItem icon={<SlidersHorizontal size={14} />} onSelect={() => setZones("position")}>
                  Split a knob into zones…
                </ContextMenuItem>
                <ContextMenuItem icon={<Gauge size={14} />} onSelect={() => setZones("velocity")}>
                  Split a pad by how hard it is hit…
                </ContextMenuItem>
              </>
            )}
          </ContextMenuContent>
        </ContextMenu>
      )}
    </div>
  );
}

/** What a right click offers while several mappings are picked. */
function BulkMenu({ ids, otherProfiles }: { ids: string[]; otherProfiles: Profile[] }) {
  const mappings = pickedMappings(ids);
  return (
    <>
      <ContextMenuItem icon={<Power size={14} />} disabled={mappings.every((m) => m.enabled)} onSelect={() => bulk.setEnabled(ids, true)}>
        Enable {plural(ids.length, "mapping")}
      </ContextMenuItem>
      <ContextMenuItem icon={<Power size={14} />} disabled={mappings.every((m) => !m.enabled)} onSelect={() => bulk.setEnabled(ids, false)}>
        Disable {plural(ids.length, "mapping")}
      </ContextMenuItem>
      <ContextMenuItem icon={<Copy size={14} />} shortcut="Ctrl+D" onSelect={() => bulk.duplicate(ids)}>
        Duplicate
      </ContextMenuItem>
      <ContextMenuItem icon={<ClipboardCopy size={14} />} shortcut="Ctrl+C" onSelect={() => bulk.copy(ids)}>
        Copy
      </ContextMenuItem>
      {otherProfiles.length > 0 && (
        <>
          <ContextMenuSeparator />
          <ContextMenuSub>
            <ContextMenuSubTrigger icon={<FolderInput size={14} />}>Move to profile</ContextMenuSubTrigger>
            <ContextMenuSubContent>
              {otherProfiles.map((p) => (
                <ContextMenuItem key={p.id} onSelect={() => bulk.moveTo(ids, p)}>
                  {p.name}
                </ContextMenuItem>
              ))}
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuSub>
            <ContextMenuSubTrigger icon={<FolderPlus size={14} />}>Copy to profile</ContextMenuSubTrigger>
            <ContextMenuSubContent>
              {otherProfiles.map((p) => (
                <ContextMenuItem key={p.id} onSelect={() => bulk.copyTo(ids, p)}>
                  {p.name}
                </ContextMenuItem>
              ))}
            </ContextMenuSubContent>
          </ContextMenuSub>
        </>
      )}
      <ContextMenuSeparator />
      <ContextMenuItem tone="danger" icon={<Trash2 size={14} />} shortcut="Del" onSelect={() => bulk.remove(ids)}>
        Delete {plural(ids.length, "mapping")}
      </ContextMenuItem>
    </>
  );
}

/** What a right click on a mapping offers. */
function RowMenu({ mapping, otherProfiles }: { mapping: Mapping; otherProfiles: Profile[] }) {
  const act = useApp.getState;
  const undoable = (message: string) => act().toast(message, { action: { label: "Undo", run: act().undo } });
  const siblings = act().config.profiles.find((p) => p.mappings.some((m) => m.id === mapping.id))?.mappings ?? [];
  const index = siblings.findIndex((m) => m.id === mapping.id);
  const searching = act().search !== "";
  return (
    <>
      <ContextMenuItem icon={<Pencil size={14} />} shortcut="Enter" onSelect={() => act().selectMapping(mapping.id)}>
        Edit
      </ContextMenuItem>
      <ContextMenuItem icon={<Copy size={14} />} shortcut="Ctrl+D" onSelect={() => act().cloneMapping(mapping.id)}>
        Duplicate
      </ContextMenuItem>
      <ContextMenuItem
        icon={<ClipboardCopy size={14} />}
        shortcut="Ctrl+C"
        onSelect={() =>
          void navigator.clipboard
            .writeText(mappingsToText([mapping]))
            .then(() => act().toast("Mapping copied. Paste it into another profile with Ctrl+V — or anywhere, it is plain text."))
            .catch(() => act().toast("The clipboard is not available here. Press Ctrl+C on the row instead.", { kind: "warning" }))
        }
      >
        Copy
      </ContextMenuItem>
      <ContextMenuItem
        icon={<Power size={14} />}
        onSelect={() => act().updateMapping(mapping.id, (m) => ({ ...m, enabled: !m.enabled }))}
      >
        {mapping.enabled ? "Disable" : "Enable"}
      </ContextMenuItem>
      {!searching && siblings.length > 1 && (
        <>
          <ContextMenuItem
            icon={<ArrowUp size={14} />}
            shortcut="Alt+↑"
            disabled={index <= 0}
            onSelect={() => act().moveMapping(mapping.id, index - 1)}
          >
            Move up
          </ContextMenuItem>
          <ContextMenuItem
            icon={<ArrowDown size={14} />}
            shortcut="Alt+↓"
            disabled={index >= siblings.length - 1}
            onSelect={() => act().moveMapping(mapping.id, index + 1)}
          >
            Move down
          </ContextMenuItem>
        </>
      )}
      {otherProfiles.length > 0 && (
        <>
          <ContextMenuSeparator />
          <ContextMenuSub>
            <ContextMenuSubTrigger icon={<FolderInput size={14} />}>Move to profile</ContextMenuSubTrigger>
            <ContextMenuSubContent>
              {otherProfiles.map((p) => (
                <ContextMenuItem
                  key={p.id}
                  onSelect={() => {
                    act().moveMappingToProfile(mapping.id, p.id);
                    undoable(`Moved to “${p.name}”`);
                  }}
                >
                  {p.name}
                </ContextMenuItem>
              ))}
            </ContextMenuSubContent>
          </ContextMenuSub>
          <ContextMenuSub>
            <ContextMenuSubTrigger icon={<FolderPlus size={14} />}>Copy to profile</ContextMenuSubTrigger>
            <ContextMenuSubContent>
              {otherProfiles.map((p) => (
                <ContextMenuItem
                  key={p.id}
                  onSelect={() => {
                    act().copyMappingToProfile(mapping.id, p.id);
                    undoable(`Copied to “${p.name}”`);
                  }}
                >
                  {p.name}
                </ContextMenuItem>
              ))}
            </ContextMenuSubContent>
          </ContextMenuSub>
        </>
      )}
      <ContextMenuSeparator />
      <ContextMenuItem
        tone="danger"
        icon={<Trash2 size={14} />}
        shortcut="Del"
        onSelect={() => {
          act().removeMapping(mapping.id);
          undoable("Mapping deleted");
        }}
      >
        Delete
      </ContextMenuItem>
    </>
  );
}

function EmptyState({ onAdd }: { onAdd: () => void }) {
  const octave = useOctave();
  // The app answers the very first touch: whatever control is moved is offered for mapping.
  const [touched, setTouched] = useState<MidiLog | null>(null);
  useEffect(
    () =>
      onMidi((log) => {
        const message = log.message;
        if (message.type === "note_off" || (message.type === "note_on" && message.velocity === 0)) return;
        if (sourceForMessage(message, null)) setTouched(log);
      }),
    [],
  );
  const touchedSource = touched && sourceForMessage(touched.message, null);
  return (
    <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-5 p-8 text-center">
      <div className="flex items-center gap-3 text-fg-faint">
        <span className="flex h-[52px] items-center gap-2 rounded-xl border border-midi-line bg-midi-soft px-4 text-xl font-semibold text-midi">
          C4
        </span>
        <ArrowRight size={20} />
        <kbd className="keycap h-[46px] min-w-[46px] text-xl">↑</kbd>
      </div>
      <div>
        <div className="text-xl font-semibold">Map your first control</div>
        <div className="mx-auto mt-1.5 max-w-[420px] text-fg-muted">
          Click <span className="font-medium text-fg">Add mapping</span>, touch something on your MIDI
          controller, then press the key or shortcut it should send. That’s it.
        </div>
      </div>
      {touched && touchedSource ? (
        <Button variant="midi" size="lg" className="animate-pop-in" icon={<Plus size={16} />} onClick={() => mapMessage(touched)}>
          You touched {sourceTitle(touchedSource.event, octave)} — map it
        </Button>
      ) : (
        <Button variant="primary" size="lg" icon={<Plus size={16} />} onClick={onAdd}>
          Add mapping
        </Button>
      )}
      <div className="flex items-center gap-2 text-sm text-fg-faint">
        or press <Kbd>N</Kbd>
        <span>·</span>
        <Menu>
          <MenuTrigger className="underline underline-offset-2 hover:text-fg">start from an example</MenuTrigger>
          <MenuContent align="center" className="max-h-[360px] w-[300px] overflow-y-auto">
            {EXAMPLES.map((example) => (
              <MenuItem key={example.file} className="h-auto py-1" onSelect={() => void importExample(example)}>
                <div className="min-w-0">
                  <div className="truncate">{example.title}</div>
                  <div className="truncate text-sm text-fg-faint">{example.detail}</div>
                </div>
              </MenuItem>
            ))}
          </MenuContent>
        </Menu>
      </div>
    </div>
  );
}
