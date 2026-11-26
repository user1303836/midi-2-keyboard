import { CircleDot } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import type { Key } from "@/bindings/Key";
import type { Mapping } from "@/bindings/Mapping";
import { Button } from "@/components/ui/button";
import { NumberInput, Segmented } from "@/components/ui/inputs";
import { Field, Keycap } from "@/components/ui/misc";
import { Dialog } from "@/components/ui/overlays";
import { ccName } from "@/lib/midi";
import { newMapping, newSource } from "@/lib/model";
import { onMidi } from "@/lib/monitor";
import { api, useApp } from "@/lib/store";
import { cn, plural } from "@/lib/utils";

import { ChannelSelect } from "../editor/fields";
import { KeySequenceInput } from "./NoteRunDialog";

type Mode = "tap" | "hold";

/**
 * "This knob is a selector switch": splits the travel of a knob or fader into equal zones and
 * creates one mapping per zone — a tool selector, a gear lever, a three-position switch.
 */
export function ZonesDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const addMappings = useApp((state) => state.addMappings);
  const toast = useApp((state) => state.toast);
  const pinDevice = useApp((state) => state.ports.filter((p) => p.state === "connected").length > 1);
  const [controller, setController] = useState(1);
  const [channel, setChannel] = useState<number | null>(null);
  const [device, setDevice] = useState<string | null>(null);
  const [keys, setKeys] = useState<Key[]>([]);
  const [mode, setMode] = useState<Mode>("tap");
  const [learning, setLearning] = useState(false);
  /** Where the knob is right now, once it has been seen. */
  const [position, setPosition] = useState<number | null>(null);

  // The dialog listens for the knob from the moment it opens (and never after it closed).
  useEffect(() => {
    if (open) {
      setKeys([]);
      setPosition(null);
      setDevice(null);
    }
    setLearning(open);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    if (learning) void api().then((backend) => backend.setLearning(true));
    const off = onMidi((log) => {
      const message = log.message;
      if (message.type !== "control_change") return;
      if (learning) {
        setController(message.controller);
        setChannel(message.channel);
        setDevice(pinDevice ? log.device : null);
        setPosition(message.value);
        setLearning(false);
      } else if (message.controller === controller && (channel === null || message.channel === channel)) {
        setPosition(message.value);
      }
    });
    return () => {
      off();
      if (learning) void api().then((backend) => backend.setLearning(false));
    };
  }, [open, learning, controller, channel, pinDevice]);

  // One value between neighbouring zones belongs to neither. Together with the one step of dead
  // zone each mapping gets, that makes the hand-over exact: going up, the old zone lets go on
  // the very value the new one takes over, and the same going down — so exactly one zone is
  // ever "the current one", and a knob resting on a border cannot flicker between two.
  const zones = useMemo(
    () =>
      keys.map((key, index) => ({
        key,
        min: Math.floor((index * 128) / keys.length) + (index > 0 ? 1 : 0),
        max: Math.floor(((index + 1) * 128) / keys.length) - 1,
      })),
    [keys],
  );

  const create = () => {
    const mappings: Mapping[] = zones.map(({ key, min, max }) =>
      newMapping(
        {
          ...newSource({ type: "control_change", controller, range: { min, max }, encoding: "absolute" }),
          channel,
          device,
          // See `zones`: the dead zone that makes neighbouring zones hand over cleanly.
          hysteresis: zones.length > 1 ? 1 : 0,
        },
        mode === "hold"
          ? { type: "hold", target: { type: "keys", keys: [key] }, auto_repeat: false }
          : { type: "trigger", on_press: { type: "keys", keys: [key], tap_ms: null }, on_release: null, repeat: null },
      ),
    );
    addMappings(mappings);
    onOpenChange(false);
    toast(`Added ${plural(mappings.length, "mapping")}: CC ${controller} is now a ${mappings.length}-position selector`, { kind: "success" });
  };

  const name = ccName(controller);

  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Split a knob into zones"
      description="Turns a knob or fader into a selector switch: each stretch of its travel gets its own key. One mapping per zone, created in one go."
      width={600}
      footer={
        <>
          <span className="mr-auto text-sm text-fg-faint">
            {keys.length > 64
              ? "That is more zones than a knob has room for — 64 at most."
              : keys.length
                ? `${plural(keys.length, "mapping")} will be added to this profile.`
                : ""}
          </span>
          <Button onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button variant="primary" disabled={keys.length < 2 || keys.length > 64} onClick={create}>
            Add {keys.length >= 2 ? plural(keys.length, "mapping") : "mappings"}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field
          label="Knob"
          hint={learning ? "Move the knob or fader now — or type its controller number." : name ? `CC ${controller} · ${name}` : `CC ${controller}`}
        >
          <div className="flex flex-wrap items-center gap-2">
            <NumberInput
              aria-label="Controller number"
              className="w-[76px]"
              min={0}
              max={127}
              value={controller}
              onChange={(next) => {
                setController(next);
                setLearning(false);
                setPosition(null);
              }}
            />
            <Button
              variant={learning ? "midi" : "secondary"}
              icon={<CircleDot size={13} className={learning ? "animate-pulse" : undefined} />}
              onClick={() => setLearning(!learning)}
            >
              {learning ? "Move the knob…" : "Learn"}
            </Button>
            <ChannelSelect value={channel} onChange={setChannel} />
          </div>
        </Field>
        <Field label="Keys" align="start" hint="One key per zone, from the bottom of the knob's travel to the top.">
          <KeySequenceInput keys={keys} onChange={setKeys} first="the bottom zone first" />
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
          label="Each zone"
          hint={
            mode === "tap"
              ? "The key is tapped once when the knob arrives in the zone."
              : "The key stays down for as long as the knob is in the zone."
          }
        >
          <Segmented<Mode>
            value={mode}
            options={[
              { value: "tap", label: "Taps its key on arrival" },
              { value: "hold", label: "Holds its key" },
            ]}
            onChange={setMode}
          />
        </Field>

        {zones.length >= 2 && zones.length <= 64 && (
          <div>
            <div className="relative flex h-[46px] overflow-hidden rounded-lg border bg-app">
              {zones.map((zone, index) => {
                const here = position !== null && position >= zone.min && position <= zone.max;
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
              {position !== null && (
                <span
                  className="pointer-events-none absolute top-0 bottom-0 w-[2px] bg-midi shadow-[0_0_6px_var(--midi)]"
                  style={{ left: `${(position / 127) * 100}%` }}
                />
              )}
            </div>
            <div className="mt-1 text-sm text-fg-faint">
              {position === null ? "Move the knob to see where it is." : `The knob is at ${position}.`} The value between two
              zones belongs to neither, so a knob resting on a border does not flicker.
            </div>
          </div>
        )}
      </div>
    </Dialog>
  );
}
