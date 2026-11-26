import { CircleDot, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";

import type { Key } from "@/bindings/Key";
import type { Mapping } from "@/bindings/Mapping";
import { Button } from "@/components/ui/button";
import { Segmented } from "@/components/ui/inputs";
import { Field, Keycap } from "@/components/ui/misc";
import { Dialog } from "@/components/ui/overlays";
import { keyFromEvent } from "@/lib/keys";
import { isBlackKey, noteName } from "@/lib/midi";
import { newMapping, newSource } from "@/lib/model";
import { onMidi } from "@/lib/monitor";
import { api, useApp, useOctave } from "@/lib/store";
import { cn, plural } from "@/lib/utils";

import { ChannelSelect, NoteInput } from "../editor/fields";

type Scale = "all" | "white" | "black";
type Mode = "hold" | "tap";

/** Captures keys one after another, in the order they are pressed (not as a combination). */
export function KeySequenceInput({ keys, onChange }: { keys: Key[]; onChange: (keys: Key[]) => void }) {
  const [armed, setArmed] = useState(true);
  const ref = useRef<HTMLDivElement>(null);
  const latest = useRef({ keys, onChange });
  latest.current = { keys, onChange };

  useEffect(() => {
    if (!armed) return;
    const down = (event: KeyboardEvent) => {
      // Tab and Escape keep their meaning here so the dialog stays usable from the keyboard.
      if (event.key === "Tab" || event.key === "Escape") {
        setArmed(false);
        return;
      }
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat) return;
      if (event.key === "Backspace") {
        latest.current.onChange(latest.current.keys.slice(0, -1));
        return;
      }
      const key = keyFromEvent(event);
      if (key) latest.current.onChange([...latest.current.keys, key]);
    };
    const pointer = (event: PointerEvent) => {
      if (!ref.current?.contains(event.target as Node)) setArmed(false);
    };
    window.addEventListener("keydown", down, true);
    window.addEventListener("pointerdown", pointer, true);
    return () => {
      window.removeEventListener("keydown", down, true);
      window.removeEventListener("pointerdown", pointer, true);
    };
  }, [armed]);

  return (
    <div
      ref={ref}
      role="button"
      tabIndex={0}
      aria-label="Keys, in order"
      onClick={() => setArmed(true)}
      onKeyDown={(event) => {
        if (!armed && (event.key === "Enter" || event.key === " ")) {
          event.preventDefault();
          setArmed(true);
        }
      }}
      className={cn(
        "flex min-h-[64px] cursor-text flex-wrap content-start items-center gap-1 rounded-md border bg-app p-2 transition-colors",
        armed ? "animate-listen border-key ring-2 ring-key-soft [--ring-color:var(--key-line)]" : "border-line-strong hover:border-fg-faint",
      )}
    >
      {keys.map((key, index) => (
        <Keycap key={`${key}-${index}`} k={key} small />
      ))}
      <span className={cn("px-1 text-sm", armed ? "text-key" : "text-fg-faint")}>
        {armed
          ? keys.length
            ? "keep typing… (Backspace removes the last)"
            : "Type the keys in order, lowest note first…"
          : keys.length
            ? "click to add more"
            : "Click, then type the keys in order"}
      </span>
    </div>
  );
}

/**
 * "This run of piano keys plays that row of computer keys": creates one mapping per note in a
 * single step. The classic uses are in-game instruments and turning a keyboard or pad grid into
 * a set of held game keys.
 */
export function NoteRunDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const octave = useOctave();
  const addMappings = useApp((state) => state.addMappings);
  const toast = useApp((state) => state.toast);
  const [first, setFirst] = useState(60);
  const [channel, setChannel] = useState<number | null>(null);
  const [keys, setKeys] = useState<Key[]>([]);
  const [scale, setScale] = useState<Scale>("white");
  const [mode, setMode] = useState<Mode>("hold");
  const [learning, setLearning] = useState(false);

  // The dialog listens for the first note from the moment it opens (and never after it closed).
  useEffect(() => {
    if (open) setKeys([]);
    setLearning(open);
  }, [open]);

  // Learn the first note by playing it.
  useEffect(() => {
    if (!learning) return;
    void api().then((backend) => backend.setLearning(true));
    const off = onMidi((log) => {
      if (log.message.type === "note_on" && log.message.velocity > 0) {
        setFirst(log.message.note);
        setChannel(log.message.channel);
        setLearning(false);
      }
    });
    return () => {
      off();
      void api().then((backend) => backend.setLearning(false));
    };
  }, [learning]);

  const notes = useMemo(() => {
    const result: number[] = [];
    for (let note = first; note <= 127 && result.length < keys.length; note++) {
      if (scale === "white" && isBlackKey(note)) continue;
      if (scale === "black" && !isBlackKey(note)) continue;
      result.push(note);
    }
    return result;
  }, [first, keys.length, scale]);

  const create = () => {
    const mappings: Mapping[] = notes.map((note, index) => {
      const key = keys[index];
      const source = {
        ...newSource({ type: "note", notes: { min: note, max: note }, velocity: { min: 1, max: 127 } }),
        channel,
      };
      return newMapping(
        source,
        mode === "hold"
          ? { type: "hold", target: { type: "keys", keys: [key] }, auto_repeat: true }
          : { type: "trigger", on_press: { type: "keys", keys: [key], tap_ms: null }, on_release: null, repeat: null },
      );
    });
    addMappings(mappings);
    onOpenChange(false);
    toast(`Added ${plural(mappings.length, "mapping")}`, { kind: "success" });
  };

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Map a run of notes"
      description="One mapping per note, created in one go: play a row of computer keys from a stretch of your MIDI keyboard or pads."
      width={600}
      footer={
        <>
          <span className="mr-auto text-sm text-fg-faint">
            {notes.length < keys.length
              ? `Only ${notes.length} of ${keys.length} keys fit before the top of the MIDI range.`
              : keys.length
                ? `${plural(keys.length, "mapping")} will be added to this profile.`
                : ""}
          </span>
          <Button onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button variant="primary" disabled={notes.length === 0} onClick={create}>
            Add {notes.length > 0 ? plural(notes.length, "mapping") : "mappings"}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field
          label="Start at"
          hint={
            learning
              ? "Play the lowest note of the run now — or type it."
              : "The lowest note of the run. Click Learn to play it again, or type it."
          }
        >
          <div className="flex flex-wrap items-center gap-2">
            <NoteInput
              value={first}
              onChange={(note) => {
                setFirst(note);
                setLearning(false);
              }}
            />
            <Button
              variant={learning ? "midi" : "secondary"}
              icon={<CircleDot size={13} className={learning ? "animate-pulse" : undefined} />}
              onClick={() => setLearning(!learning)}
            >
              {learning ? "Play the note…" : "Learn"}
            </Button>
            <ChannelSelect value={channel} onChange={setChannel} />
          </div>
        </Field>
        <Field label="Use" hint="Game instruments usually map only the white keys of a scale.">
          <Segmented<Scale>
            value={scale}
            options={[
              { value: "white", label: "White keys only" },
              { value: "all", label: "Every key" },
              { value: "black", label: "Black keys only" },
            ]}
            onChange={setScale}
          />
        </Field>
        <Field label="Keys" align="start">
          <KeySequenceInput keys={keys} onChange={setKeys} />
          {keys.length > 0 && (
            <button
              type="button"
              className="mt-1 text-sm text-fg-faint underline-offset-2 hover:text-fg hover:underline"
              onClick={() => setKeys([])}
            >
              Start over
            </button>
          )}
        </Field>
        <Field
          label="Each note"
          hint={
            mode === "hold"
              ? "The key stays down exactly as long as the note is held — like a real keyboard, and what games expect."
              : "The key is tapped once per note, however long the note is held."
          }
        >
          <Segmented<Mode>
            value={mode}
            options={[
              { value: "hold", label: "Holds its key" },
              { value: "tap", label: "Taps its key" },
            ]}
            onChange={setMode}
          />
        </Field>

        {notes.length > 0 && (
          <div className="flex max-h-[130px] flex-wrap gap-1.5 overflow-y-auto rounded-lg border bg-app p-2.5">
            {notes.map((note, index) => (
              <span key={note} className="inline-flex items-center gap-1 rounded-md border px-1.5 py-[3px]">
                <span className="font-semibold text-midi">{noteName(note, octave)}</span>
                <span className="text-fg-faint">→</span>
                <Keycap k={keys[index]} small />
                <button
                  type="button"
                  aria-label={`Remove ${noteName(note, octave)}`}
                  className="text-fg-faint hover:text-fg"
                  onClick={() => setKeys(keys.filter((_, i) => i !== index))}
                >
                  <X size={11} />
                </button>
              </span>
            ))}
          </div>
        )}
      </div>
    </Dialog>
  );
}
