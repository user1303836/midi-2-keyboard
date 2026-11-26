import { ArrowRight, Copy, Filter, GripVertical, MoreHorizontal, Plus, Search, Timer, Trash2, X } from "lucide-react";
import { memo, useEffect, useMemo, useRef, useState } from "react";

import type { Mapping } from "@/bindings/Mapping";
import type { Profile } from "@/bindings/Profile";
import { Button, IconButton } from "@/components/ui/button";
import { Switch } from "@/components/ui/inputs";
import { Badge, Kbd } from "@/components/ui/misc";
import { Menu, MenuContent, MenuItem, MenuSeparator, MenuTrigger, Tooltip } from "@/components/ui/overlays";
import { conditionText, mappingSentence, rateLimitText, sourceNickname, sourceQualifiers, sourceTitle } from "@/lib/describe";
import { newMapping } from "@/lib/model";
import { onMappingActivity, type ActivityKind } from "@/lib/monitor";
import { useApp, useOctave } from "@/lib/store";
import { cn, plural } from "@/lib/utils";

import { BehaviorSummary, SourceIcon } from "./tokens";

/** The little light on each row: teal when its source matched, amber when its action went out. */
function ActivityLed({ mappingId }: { mappingId: string }) {
  const ref = useRef<HTMLSpanElement>(null);
  useEffect(
    () =>
      onMappingActivity(mappingId, (kind: ActivityKind) => {
        const element = ref.current;
        if (!element) return;
        const color =
          kind === "sent" ? "var(--key)" : kind === "failed" ? "var(--danger)" : kind === "matched" ? "var(--midi)" : "var(--fg-faint)";
        element.style.background = color;
        element.style.boxShadow = `0 0 8px ${color}`;
        element.classList.remove("animate-flash");
        void element.offsetWidth; // restart the animation
        element.classList.add("animate-flash");
      }),
    [mappingId],
  );
  return (
    <span className="relative flex size-[8px] items-center justify-center">
      <span className="absolute size-[6px] rounded-full bg-line-strong" />
      <span ref={ref} className="absolute size-[8px] rounded-full opacity-0" />
    </span>
  );
}

const MappingRow = memo(function MappingRow({
  mapping,
  selected,
  engaged,
  dragging,
  dropBefore,
  onDragStart,
  onDragOver,
  onDrop,
  onDragEnd,
  draggable,
}: {
  mapping: Mapping;
  selected: boolean;
  engaged: boolean;
  dragging: boolean;
  dropBefore: boolean;
  onDragStart: () => void;
  onDragOver: (event: React.DragEvent) => void;
  onDrop: () => void;
  onDragEnd: () => void;
  draggable: boolean;
}) {
  const selectMapping = useApp((state) => state.selectMapping);
  const updateMapping = useApp((state) => state.updateMapping);
  const removeMapping = useApp((state) => state.removeMapping);
  const cloneMapping = useApp((state) => state.cloneMapping);
  const config = useApp((state) => state.config);
  const octave = useOctave();
  const toast = useApp((state) => state.toast);
  const undo = useApp((state) => state.undo);

  const event = mapping.source.event;
  const qualifiers = sourceQualifiers(mapping.source, config);
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
        onDragStart();
      }}
      onDragOver={onDragOver}
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
      title={mappingSentence(mapping, config, octave)}
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
      <div className={cn("flex w-[236px] shrink-0 items-center gap-2", !mapping.enabled && "opacity-45")}>
        <span className="flex h-[26px] shrink-0 items-center gap-1.5 rounded-md border border-midi-line bg-midi-soft px-2 font-semibold text-midi">
          <SourceIcon event={event} size={14} />
          <span className="whitespace-nowrap">{sourceTitle(event, octave)}</span>
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
        {mapping.rate_limit && (
          <Badge tone="accent" title="Rate limit">
            <Timer size={11} />
            {rateLimitText(mapping.rate_limit)}
          </Badge>
        )}
        {mapping.conditions.length > 0 && (
          <Badge tone="neutral" title={mapping.conditions.map((c) => conditionText(c, config, octave)).join("\n")}>
            <Filter size={11} />
            {mapping.conditions.length === 1 ? conditionText(mapping.conditions[0], config, octave) : `${mapping.conditions.length} conditions`}
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
  const addMapping = useApp((state) => state.addMapping);
  const moveMapping = useApp((state) => state.moveMapping);
  const config = useApp((state) => state.config);
  const octave = useOctave();
  const [dragId, setDragId] = useState<string | null>(null);
  const [dropIndex, setDropIndex] = useState<number | null>(null);

  const visible = useMemo(
    () =>
      profile.mappings.filter((mapping) => matchesSearch(mapping, search, mappingSentence(mapping, config, octave))),
    [profile.mappings, search, config, octave],
  );

  const quickAdd = () => addMapping(newMapping(), { wizard: true });
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
        <Button variant="primary" icon={<Plus size={15} />} onClick={quickAdd}>
          Add mapping
        </Button>
      </div>

      {profile.mappings.length === 0 ? (
        <EmptyState onAdd={quickAdd} />
      ) : (
        <div
          role="grid"
          aria-label="Mappings"
          className="min-h-0 flex-1 overflow-y-auto px-2 py-2"
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
              selected={mapping.id === selectedId}
              engaged={engaged.includes(mapping.id)}
              draggable={reorderable}
              dragging={dragId === mapping.id}
              dropBefore={dropIndex === index && dragId !== null && dragId !== mapping.id}
              onDragStart={() => setDragId(mapping.id)}
              onDragOver={(e) => {
                if (!dragId) return;
                e.preventDefault();
                const rect = e.currentTarget.getBoundingClientRect();
                const after = e.clientY > rect.top + rect.height / 2;
                setDropIndex(after ? index + 1 : index);
              }}
              onDrop={() => {
                if (dragId && dropIndex !== null) {
                  const from = profile.mappings.findIndex((m) => m.id === dragId);
                  moveMapping(dragId, dropIndex > from ? dropIndex - 1 : dropIndex);
                }
                setDragId(null);
                setDropIndex(null);
              }}
              onDragEnd={() => {
                setDragId(null);
                setDropIndex(null);
              }}
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
      <div className="text-sm text-fg-faint">
        or press <Kbd>N</Kbd>
      </div>
    </div>
  );
}
