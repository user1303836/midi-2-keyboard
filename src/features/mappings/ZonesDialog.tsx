import { CircleDot } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import type { Key } from "@/bindings/Key";
import type { Mapping } from "@/bindings/Mapping";
import { Button } from "@/components/ui/button";
import { NumberInput, Segmented } from "@/components/ui/inputs";
import { Field, Keycap } from "@/components/ui/misc";
import { Dialog } from "@/components/ui/overlays";
import { ccName, noteName } from "@/lib/midi";
import { newMapping, newSource } from "@/lib/model";
import { onMidi } from "@/lib/monitor";
import { api, useApp, useOctave } from "@/lib/store";
import { cn, plural } from "@/lib/utils";

import { ChannelSelect, NoteInput } from "../editor/fields";
import { KeySequenceInput } from "./NoteRunDialog";

type Mode = "tap" | "hold";

/** What is split into zones: where a knob or fader stands, or how hard a pad or key is hit. */
export type ZoneKind = "position" | "velocity";

/**
 * One control, several jobs, chosen by a value:
 * - **position**: "this knob is a selector switch" — the travel of a knob or fader in equal
 *   zones, one mapping per zone (a tool selector, a gear lever, a three-position switch);
 * - **velocity**: "soft and hard hits do different things" — the velocity of one pad or key in
 *   equal bands, one mapping per band.
 */
export function ZonesDialog({
  kind,
  open,
  onOpenChange,
}: {
  kind: ZoneKind;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const addMappings = useApp((state) => state.addMappings);
  const toast = useApp((state) => state.toast);
  const pinDevice = useApp((state) => state.ports.filter((p) => p.state === "connected").length > 1);
  const octave = useOctave();
  /** The controller number (position) or the note number (velocity). */
  const [number, setNumber] = useState(kind === "position" ? 1 : 60);
  const [channel, setChannel] = useState<number | null>(null);
  const [device, setDevice] = useState<string | null>(null);
  const [keys, setKeys] = useState<Key[]>([]);
  const [mode, setMode] = useState<Mode>("tap");
  const [learning, setLearning] = useState(false);
  /** The knob's position, or the velocity of the last hit, once one has been seen. */
  const [value, setValue] = useState<number | null>(null);
  const position = kind === "position";
  const lowest = position ? 0 : 1;

  // The dialog listens for the control from the moment it opens (and never after it closed).
  useEffect(() => {
    if (open) {
      setKeys([]);
      setValue(null);
      setDevice(null);
    }
    setLearning(open);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    if (learning) void api().then((backend) => backend.setLearning(true));
    const off = onMidi((log) => {
      const message = log.message;
      const seen =
        position && message.type === "control_change"
          ? { number: message.controller, value: message.value }
          : !position && message.type === "note_on" && message.velocity > 0
            ? { number: message.note, value: message.velocity }
            : null;
      if (!seen) return;
      if (learning) {
        setNumber(seen.number);
        setChannel(message.type === "control_change" || message.type === "note_on" ? message.channel : null);
        setDevice(pinDevice ? log.device : null);
        setValue(seen.value);
        setLearning(false);
      } else if (seen.number === number && (channel === null || ("channel" in message && message.channel === channel))) {
        setValue(seen.value);
      }
    });
    return () => {
      off();
      if (learning) void api().then((backend) => backend.setLearning(false));
    };
  }, [open, learning, number, channel, pinDevice, position]);

  // Position: one value between neighbouring zones belongs to neither. Together with the one
  // step of dead zone each mapping gets, that makes the hand-over exact: going up, the old zone
  // lets go on the very value the new one takes over, and the same going down — so exactly one
  // zone is ever "the current one", and a knob resting on a border cannot flicker between two.
  // Velocity: every hit stands for itself, so the bands simply meet (1–127; 0 is "note off").
  const zones = useMemo(() => {
    const span = 128 - lowest;
    return keys.map((key, index) => ({
      key,
      min: lowest + Math.floor((index * span) / keys.length) + (position && index > 0 ? 1 : 0),
      max: lowest + Math.floor(((index + 1) * span) / keys.length) - 1,
    }));
  }, [keys, position, lowest]);
  const most = position ? 64 : 127;

  const create = () => {
    const mappings: Mapping[] = zones.map(({ key, min, max }) =>
      newMapping(
        position
          ? {
              ...newSource({ type: "control_change", controller: number, range: { min, max }, encoding: "absolute" }),
              channel,
              device,
              // See `zones`: the dead zone that makes neighbouring zones hand over cleanly.
              hysteresis: zones.length > 1 ? 1 : 0,
            }
          : { ...newSource({ type: "note", notes: { min: number, max: number }, velocity: { min, max } }), channel, device },
        mode === "hold"
          ? { type: "hold", target: { type: "keys", keys: [key] }, auto_repeat: false }
          : { type: "trigger", on_press: { type: "keys", keys: [key], tap_ms: null }, on_release: null, repeat: null },
      ),
    );
    addMappings(mappings);
    onOpenChange(false);
    toast(
      position
        ? `Added ${plural(mappings.length, "mapping")}: CC ${number} is now a ${mappings.length}-position selector`
        : `Added ${plural(mappings.length, "mapping")}: ${noteName(number, octave)} now does ${mappings.length} things, by how hard it is hit`,
      { kind: "success" },
    );
  };

  const name = position ? ccName(number) : "";

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title={position ? "Split a knob into zones" : "Split a pad by how hard it is hit"}
      description={
        position
          ? "Turns a knob or fader into a selector switch: each stretch of its travel gets its own key. One mapping per zone, created in one go."
          : "Soft and hard hits do different things: each band of velocity gets its own key. One mapping per band, created in one go."
      }
      width={600}
      footer={
        <>
          <span className="mr-auto text-sm text-fg-faint">
            {keys.length > most
              ? `That is more than there is room for — ${most} at most.`
              : keys.length
                ? `${plural(keys.length, "mapping")} will be added to this profile.`
                : ""}
          </span>
          <Button onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button variant="primary" disabled={keys.length < 2 || keys.length > most} onClick={create}>
            Add {keys.length >= 2 ? plural(keys.length, "mapping") : "mappings"}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field
          label={position ? "Knob" : "Pad or key"}
          hint={
            learning
              ? position
                ? "Move the knob or fader now — or type its controller number."
                : "Hit the pad or key now — or type its note."
              : position
                ? name
                  ? `CC ${number} · ${name}`
                  : `CC ${number}`
                : undefined
          }
        >
          <div className="flex flex-wrap items-center gap-2">
            {position ? (
              <NumberInput
                aria-label="Controller number"
                className="w-[76px]"
                min={0}
                max={127}
                value={number}
                onChange={(next) => {
                  setNumber(next);
                  setLearning(false);
                  setValue(null);
                }}
              />
            ) : (
              <NoteInput
                aria-label="Note to split"
                value={number}
                onChange={(next) => {
                  setNumber(next);
                  setLearning(false);
                  setValue(null);
                }}
              />
            )}
            <Button
              variant={learning ? "midi" : "secondary"}
              icon={<CircleDot size={13} className={learning ? "animate-pulse" : undefined} />}
              onClick={() => setLearning(!learning)}
            >
              {learning ? (position ? "Move the knob…" : "Hit the pad…") : "Learn"}
            </Button>
            <ChannelSelect value={channel} onChange={setChannel} />
          </div>
        </Field>
        <Field
          label="Keys"
          align="start"
          hint={
            position
              ? "One key per zone, from the bottom of the knob's travel to the top."
              : "One key per band, from the softest hit to the hardest."
          }
        >
          <KeySequenceInput keys={keys} onChange={setKeys} first={position ? "the bottom zone first" : "the softest hit first"} />
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
          label={position ? "Each zone" : "Each band"}
          hint={
            position
              ? mode === "tap"
                ? "The key is tapped once when the knob arrives in the zone."
                : "The key stays down for as long as the knob is in the zone."
              : mode === "tap"
                ? "The key is tapped once per hit."
                : "The key stays down for as long as the pad or key is held."
          }
        >
          <Segmented<Mode>
            value={mode}
            options={[
              { value: "tap", label: position ? "Taps its key on arrival" : "Taps its key" },
              { value: "hold", label: "Holds its key" },
            ]}
            onChange={setMode}
          />
        </Field>

        {zones.length >= 2 && zones.length <= most && (
          <div>
            <div className="relative flex h-[46px] overflow-hidden rounded-lg border bg-app">
              {zones.map((zone, index) => {
                const here = value !== null && value >= zone.min && value <= zone.max;
                return (
                  <div
                    key={index}
                    title={`${zone.min}–${zone.max}`}
                    className={cn(
                      "flex min-w-0 flex-1 flex-col items-center justify-center gap-[2px] border-r last:border-r-0",
                      here && "bg-midi-soft",
                    )}
                  >
                    {zones.length <= 16 && <Keycap k={zone.key} small />}
                    {zones.length <= 10 && (
                      <span className="text-2xs text-fg-faint tabular-nums">
                        {zone.min}–{zone.max}
                      </span>
                    )}
                  </div>
                );
              })}
              {value !== null && (
                <span
                  className="pointer-events-none absolute top-0 bottom-0 w-[2px] bg-midi shadow-[0_0_6px_var(--midi)]"
                  style={{ left: `${((value - lowest) / (127 - lowest)) * 100}%` }}
                />
              )}
            </div>
            <div className="mt-1 text-sm text-fg-faint">
              {position
                ? `${value === null ? "Move the knob to see where it is." : `The knob is at ${value}.`} The value between two zones belongs to neither, so a knob resting on a border does not flicker.`
                : value === null
                  ? "Hit the pad to see where your touch lands — softly, then hard."
                  : `That hit had velocity ${value}. Hit it softly, then hard, to see where your touch lands.`}
            </div>
          </div>
        )}
      </div>
    </Dialog>
  );
}
