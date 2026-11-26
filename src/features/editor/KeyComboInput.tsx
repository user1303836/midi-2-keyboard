import { Keyboard, Plus, X } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { Key } from "@/bindings/Key";
import { Popover, PopoverContent, PopoverTrigger, Tooltip } from "@/components/ui/overlays";
import { KeyCombo } from "@/components/ui/misc";
import { allKeys, KEY_GROUPS, keyFromEvent, normalizeCombo, type KeyInfo } from "@/lib/keys";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";

/**
 * Captures a key combination by pressing it.
 *
 * Click (or focus + Enter) arms the field; every key pressed until all keys are released again
 * becomes the combination, then the field disarms itself — so Tab and Enter can be bound like
 * any other key and the field never traps keyboard focus. Esc as the very first key backs out
 * instead, as everybody expects it to; Escape itself, like the keys the browser cannot see
 * (media keys, OS-reserved shortcuts), is added from the picker.
 */
export function KeyComboInput({
  keys,
  onChange,
  autoCapture = false,
  onCaptured,
  placeholder = "Click, then press a key or shortcut",
  single = false,
  className,
}: {
  keys: readonly Key[];
  onChange: (keys: Key[]) => void;
  /** Start capturing as soon as the field mounts. */
  autoCapture?: boolean;
  /** Called after a capture completed (not for picker edits). */
  onCaptured?: (keys: Key[]) => void;
  placeholder?: string;
  /** Capture exactly one key (for key-down / key-up macro steps). */
  single?: boolean;
  className?: string;
}) {
  const [capturing, setCapturing] = useState(autoCapture);
  const [live, setLive] = useState<Key[]>([]);
  const [unknown, setUnknown] = useState(false);
  // Esc just cancelled a capture the user started: say how to bind Esc itself.
  const [escNote, setEscNote] = useState(false);
  const [pickerOpen, setPickerOpen] = useState(false);
  const fieldRef = useRef<HTMLDivElement>(null);
  const latest = useRef({ onChange, onCaptured });
  latest.current = { onChange, onCaptured };

  // Armed by the quick-add flow rather than by the user: there, Esc also closes the quick-add.
  const autoArmed = useRef(autoCapture);

  useEffect(() => {
    if (!escNote) return;
    const timer = setTimeout(() => setEscNote(false), 8000);
    return () => clearTimeout(timer);
  }, [escNote]);

  // The quick-add flow arms fields one after another.
  useEffect(() => {
    if (autoCapture) {
      autoArmed.current = true;
      setCapturing(true);
    }
  }, [autoCapture]);

  const finish = useCallback((combo: Key[]) => {
    setCapturing(false);
    setLive([]);
    if (combo.length === 0) return;
    const result = normalizeCombo(combo);
    latest.current.onChange(result);
    latest.current.onCaptured?.(result);
  }, []);

  useEffect(() => {
    if (!capturing) return;
    const pressed = new Set<Key>();
    const combo: Key[] = [];
    setLive([]);
    setUnknown(false);
    setEscNote(false);

    const down = (event: KeyboardEvent) => {
      // Esc as the very first key backs out instead of binding Escape.
      if (event.key === "Escape" && combo.length === 0) {
        setCapturing(false);
        if (autoArmed.current) {
          // Nobody clicked this field, the quick-add armed it: the key is left alone so that
          // it closes the quick-add as well.
          autoArmed.current = false;
          return;
        }
        event.preventDefault();
        event.stopPropagation();
        setEscNote(true);
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      const key = keyFromEvent(event);
      if (!key) {
        setUnknown(true);
        return;
      }
      setUnknown(false);
      pressed.add(key);
      if (!combo.includes(key)) {
        combo.push(key);
        setLive(normalizeCombo(combo));
      }
      if (single) finish([key]);
    };
    const up = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      const key = keyFromEvent(event);
      if (key) {
        // Some keys (Print Screen) only ever produce a key-up.
        if (!combo.includes(key)) combo.push(key);
        pressed.delete(key);
      }
      if (pressed.size === 0 && combo.length > 0) finish(combo);
    };
    // Losing focus (the OS swallowed a shortcut, the user clicked away) ends the capture with
    // whatever was pressed so far.
    const blur = () => finish(combo);
    const pointer = (event: PointerEvent) => {
      if (!fieldRef.current?.contains(event.target as Node)) finish(combo);
    };

    window.addEventListener("keydown", down, true);
    window.addEventListener("keyup", up, true);
    window.addEventListener("blur", blur);
    window.addEventListener("pointerdown", pointer, true);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("keyup", up, true);
      window.removeEventListener("blur", blur);
      window.removeEventListener("pointerdown", pointer, true);
    };
  }, [capturing, finish, single]);

  const shown = capturing && live.length > 0 ? live : keys;

  return (
    <div className={cn("flex items-stretch gap-1", className)}>
      <div
        ref={fieldRef}
        role="button"
        tabIndex={0}
        aria-label={capturing ? "Press a key or shortcut" : "Shortcut. Press Enter to change"}
        onClick={() => {
          autoArmed.current = false;
          setCapturing(true);
        }}
        onKeyDown={(event) => {
          if (capturing) return;
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            autoArmed.current = false;
            setCapturing(true);
          } else if (event.key === "Backspace" || event.key === "Delete") {
            event.preventDefault();
            onChange([]);
          }
        }}
        className={cn(
          "group/combo flex min-h-[34px] min-w-0 flex-1 cursor-pointer flex-wrap items-center gap-1 rounded-md border bg-app px-2 py-[4px] transition-colors",
          capturing
            ? "animate-listen border-key ring-2 ring-key-soft [--ring-color:var(--key-line)]"
            : "border-line-strong hover:border-fg-faint",
        )}
      >
        {shown.length > 0 ? (
          <KeyCombo keys={shown} />
        ) : (
          <span className={cn("text-fg-faint", capturing && "text-key")}>
            {capturing ? "Press a key or shortcut…  (Esc cancels)" : placeholder}
          </span>
        )}
        {capturing && shown.length > 0 && live.length === 0 && (
          <span className="ml-1 text-sm text-key">press new keys…</span>
        )}
        {!capturing && escNote && (
          <span className="ml-1 text-sm text-fg-muted">To send Esc itself, pick it from the list →</span>
        )}
        {capturing && unknown && (
          <span className="ml-1 text-sm text-warn">That key can’t be captured — add it from the list →</span>
        )}
        {!capturing && keys.length > 0 && !single && (
          <button
            type="button"
            aria-label="Clear shortcut"
            title="Clear (Backspace)"
            onClick={(event) => {
              event.stopPropagation();
              onChange([]);
            }}
            className="ml-auto inline-flex size-[18px] shrink-0 items-center justify-center rounded text-fg-faint opacity-0 transition-opacity group-hover/combo:opacity-100 hover:bg-hover hover:text-fg focus-visible:opacity-100"
          >
            <X size={12} />
          </button>
        )}
      </div>

      <Popover open={pickerOpen} onOpenChange={setPickerOpen}>
        <Tooltip content="Pick keys from a list">
          <PopoverTrigger
            aria-label="Pick keys from a list"
            className="inline-flex w-[30px] shrink-0 items-center justify-center rounded-md border border-line-strong bg-app text-fg-muted hover:border-fg-faint hover:text-fg data-[state=open]:border-accent"
          >
            {keys.length > 0 ? <Plus size={15} /> : <Keyboard size={15} />}
          </PopoverTrigger>
        </Tooltip>
        <PopoverContent align="end" className="w-[440px] p-0">
          <KeyPicker
            selected={keys}
            single={single}
            onToggle={(key) => {
              if (single) {
                onChange([key]);
                setPickerOpen(false);
                return;
              }
              onChange(keys.includes(key) ? keys.filter((k) => k !== key) : normalizeCombo([...keys, key]));
            }}
          />
        </PopoverContent>
      </Popover>
    </div>
  );
}

function KeyPicker({
  selected,
  onToggle,
  single,
}: {
  selected: readonly Key[];
  onToggle: (key: Key) => void;
  single: boolean;
}) {
  const [query, setQuery] = useState("");
  const unsupported = useApp((state) => state.unsupportedKeys);
  const keys = useMemo(() => allKeys(), []);
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return keys;
    return keys.filter((info) =>
      `${info.label} ${info.name} ${info.key} ${info.aliases ?? ""}`.toLowerCase().includes(q),
    );
  }, [keys, query]);
  const byGroup = useMemo(() => {
    const map = new Map<string, KeyInfo[]>();
    for (const info of filtered) {
      const list = map.get(info.group) ?? [];
      list.push(info);
      map.set(info.group, list);
    }
    return map;
  }, [filtered]);

  return (
    <div className="flex max-h-[380px] flex-col">
      <div className="border-b p-2">
        <input
          autoFocus
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder="Search keys…  (F13, volume, numpad, play…)"
          className="h-[28px] w-full rounded-md border border-line-strong bg-app px-2 outline-hidden placeholder:text-fg-faint focus:border-accent"
        />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto p-2">
        {filtered.length === 0 && <div className="p-4 text-center text-fg-faint">No key matches “{query}”.</div>}
        {KEY_GROUPS.filter((group) => byGroup.has(group)).map((group) => (
          <div key={group} className="mb-2 last:mb-0">
            <div className="px-1 pb-1 text-xs font-semibold tracking-wide text-fg-faint uppercase">{group}</div>
            <div className="flex flex-wrap gap-1">
              {byGroup.get(group)!.map((info) => {
                const active = selected.includes(info.key);
                const unavailable = unsupported.includes(info.key);
                return (
                  <button
                    key={info.key}
                    type="button"
                    title={unavailable ? `${info.name} — cannot be sent on this system` : info.name}
                    aria-pressed={active}
                    disabled={unavailable && !active}
                    onClick={() => onToggle(info.key)}
                    className={cn(
                      "keycap keycap-sm font-sans transition-colors hover:border-accent",
                      active && "border-accent bg-accent-soft text-accent",
                      unavailable && "border-dashed opacity-40 hover:border-line-strong",
                    )}
                  >
                    {info.label}
                  </button>
                );
              })}
            </div>
          </div>
        ))}
      </div>
      {!single && (
        <div className="border-t px-3 py-1.5 text-sm text-fg-faint">
          Click keys to add or remove them. Modifiers are always pressed first.
        </div>
      )}
    </div>
  );
}
