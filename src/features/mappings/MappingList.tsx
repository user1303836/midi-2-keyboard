import {
  ArrowRight,
  ChevronDown,
  Copy,
  CopyPlus,
  Filter,
  GripVertical,
  MoreHorizontal,
  Piano,
  Plus,
  Search,
  Timer,
  Trash2,
  X,
} from "lucide-react";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { Config } from "@/bindings/Config";
import type { Mapping } from "@/bindings/Mapping";
import type { Profile } from "@/bindings/Profile";
import { Button, IconButton } from "@/components/ui/button";
import { Switch } from "@/components/ui/inputs";
import { Badge, Kbd } from "@/components/ui/misc";
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger, Tooltip } from "@/components/ui/overlays";
import { conditionText, mappingSentence, rateLimitText, sourceNickname, sourceQualifiers, sourceTitle } from "@/lib/describe";
import { findConflicts, type Conflict } from "@/lib/conflicts";
import { EXAMPLES, importExample } from "@/lib/examples";
import { onMappingActivity, type ActivityKind } from "@/lib/monitor";
import { useApp, useOctave } from "@/lib/store";
import { cn, plural } from "@/lib/utils";

import { NoteRunDialog } from "./NoteRunDialog";
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
}: {
  mapping: Mapping;
  index: number;
  selected: boolean;
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
  const removeMapping = useApp((state) => state.removeMapping);
  const cloneMapping = useApp((state) => state.cloneMapping);
  // Only the device list (for friendly names), not the whole config.
  const devices = useApp((state) => state.config.devices);
  const octave = useOctave();
  const toast = useApp((state) => state.toast);
  const undo = useApp((state) => state.undo);

  const event = mapping.source.event;
  const qualifiers = sourceQualifiers(mapping.source, { devices } as Config);
  const nickname = sourceNickname(event);

  return (
    <div
      role="row"
      aria-selected={selected}
      tabIndex={0}
      draggable={draggable}
      onDragStart={(e) => {
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData("text/plain", mapping.id);
        onDragStart(mapping.id);
      }}
      onDragOver={(e) => onDragOver(index, e)}
      onDrop={onDrop}
      onDragEnd={onDragEnd}
      onClick={() => selectMapping(selected ? null : mapping.id)}
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
        "group relative flex min-h-[46px] cursor-pointer items-center gap-2.5 rounded-lg border px-2.5 py-1.5 outline-hidden transition-colors",
        selected ? "border-accent-line bg-accent-soft" : "border-transparent hover:bg-hover focus-visible:border-accent-line",
        dragging && "opacity-40",
        dropBefore && "before:absolute before:-top-[3px] before:right-2 before:left-2 before:h-[2px] before:rounded-full before:bg-accent",
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
      <div className={cn("flex w-[150px] shrink-0 items-center gap-2 @min-[700px]:w-[236px]", !mapping.enabled && "opacity-45")}>
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
          <BehaviorSummary behavior={mapping.behavior} />
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
                <span className="hidden @min-[700px]:inline">shared</span>
              </Badge>
            </span>
          </Tooltip>
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
        <Tooltip content={mapping.enabled ? "Enabled" : "Disabled"}>
          <span className="flex">
            <Switch
              tone="midi"
              aria-label="Enabled"
              checked={mapping.enabled}
              onChange={(enabled) => updateMapping(mapping.id, (m) => ({ ...m, enabled }))}
            />
          </span>
        </Tooltip>
        <Menu>
          <MenuTrigger asChild>
            <IconButton label="More" size="sm" className="opacity-0 group-hover:opacity-100 data-[state=open]:opacity-100 focus-visible:opacity-100">
              <MoreHorizontal size={15} />
            </IconButton>
          </MenuTrigger>
          <MenuContent>
            <MenuItem icon={<Copy size={14} />} onSelect={() => cloneMapping(mapping.id)}>
              Duplicate
            </MenuItem>
            <MenuSeparator />
            <MenuItem
              tone="danger"
              icon={<Trash2 size={14} />}
              onSelect={() => {
                removeMapping(mapping.id);
                toast("Mapping deleted", { action: { label: "Undo", run: undo } });
              }}
            >
              Delete
            </MenuItem>
          </MenuContent>
        </Menu>
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
  const engaged = useApp((state) => state.engine.engaged_mappings);
  const search = useApp((state) => state.search);
  const setSearch = useApp((state) => state.setSearch);
  const startWizard = useApp((state) => state.startWizard);
  const moveMapping = useApp((state) => state.moveMapping);
  const config = useApp((state) => state.config);
  const octave = useOctave();
  const [runOpen, setRunOpen] = useState(false);
  const [dragId, setDragId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);
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

  const visible = useMemo(
    () =>
      profile.mappings.filter((mapping) => matchesSearch(mapping, search, mappingSentence(mapping, config, octave))),
    [profile.mappings, search, config, octave],
  );

  const conflicts = useMemo(() => findConflicts(config, profile), [config, profile]);

  const quickAdd = startWizard;
  const reorderable = search === "";

  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex h-[46px] shrink-0 items-center gap-2 border-b px-4">
        <div className="relative w-[240px]">
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
        <span className="text-sm text-fg-faint">
          {search ? `${visible.length} of ${profile.mappings.length}` : plural(profile.mappings.length, "mapping")}
        </span>
        <div className="flex-1" />
        <div className="flex">
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
            </MenuContent>
          </Menu>
        </div>
      </div>
      <NoteRunDialog open={runOpen} onOpenChange={setRunOpen} />

      {profile.mappings.length === 0 ? (
        <EmptyState onAdd={quickAdd} />
      ) : (
        <div
          role="grid"
          aria-label="Mappings"
          className="@container min-h-0 flex-1 overflow-y-auto px-2 py-2"
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
              selected={mapping.id === selectedId}
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
      )}
    </div>
  );
}

function EmptyState({ onAdd }: { onAdd: () => void }) {
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
      <Button variant="primary" size="lg" icon={<Plus size={16} />} onClick={onAdd}>
        Add mapping
      </Button>
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
