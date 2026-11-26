// Small MIDI-specific form controls shared by the editors.

import { useEffect, useState } from "react";

import type { MidiLog } from "@/bindings/MidiLog";
import type { Source } from "@/bindings/Source";
import type { ValueRange } from "@/bindings/ValueRange";
import { NumberInput, RangeSlider, Select, type SelectOption } from "@/components/ui/inputs";
import { deviceLabel } from "@/lib/describe";
import { noteName } from "@/lib/midi";
import { onMidi } from "@/lib/monitor";
import { useApp, useOctave } from "@/lib/store";
import { clamp, cn } from "@/lib/utils";

const CHANNEL_OPTIONS: SelectOption<string>[] = [
  { value: "any", label: "Any channel" },
  ...Array.from({ length: 16 }, (_, index) => ({ value: String(index + 1), label: `Channel ${index + 1}` })),
];

export function ChannelSelect({
  value,
  onChange,
  className,
}: {
  value: number | null;
  onChange: (value: number | null) => void;
  className?: string;
}) {
  return (
    <Select
      aria-label="MIDI channel"
      className={className ?? "w-[132px]"}
      value={value === null ? "any" : String(value)}
      onChange={(next) => onChange(next === "any" ? null : Number(next))}
      options={CHANNEL_OPTIONS}
    />
  );
}

export function DeviceSelect({
  value,
  onChange,
  className,
}: {
  value: string | null;
  onChange: (value: string | null) => void;
  className?: string;
}) {
  const ports = useApp((state) => state.ports);
  const config = useApp((state) => state.config);
  const names = new Set<string>();
  ports.forEach((port) => names.add(port.name));
  config.devices.forEach((device) => names.add(device.name));
  if (value) names.add(value);
  const options: SelectOption<string>[] = [
    { value: "\u0000any", label: "Any device" },
    ...Array.from(names).map((name) => ({
      value: name,
      label: deviceLabel(name, config),
      hint: ports.some((p) => p.name === name) ? undefined : "not connected",
    })),
  ];
  return (
    <Select
      aria-label="MIDI device"
      className={className}
      value={value ?? "\u0000any"}
      onChange={(next) => onChange(next === "\u0000any" ? null : next)}
      options={options}
    />
  );
}

const NOTE_LETTERS: Record<string, number> = { c: 0, d: 2, e: 4, f: 5, g: 7, a: 9, b: 11 };

/** Parses "60", "C4", "c#4", "Db3", "F♯-1" into a note number. */
export function parseNote(text: string, octave: number): number | null {
  const trimmed = text.trim().toLowerCase();
  if (/^\d+$/.test(trimmed)) {
    const value = Number(trimmed);
    return value <= 127 ? value : null;
  }
  const match = /^([a-g])\s*([#♯b♭]?)\s*(-?\d+)$/.exec(trimmed);
  if (!match) return null;
  const semitone = NOTE_LETTERS[match[1]] + (match[2] === "#" || match[2] === "♯" ? 1 : match[2] ? -1 : 0);
  const note = (Number(match[3]) + 5 - octave) * 12 + semitone;
  return note >= 0 && note <= 127 ? note : null;
}

/** A note field that accepts numbers or names and always shows both. */
export function NoteInput({
  value,
  onChange,
  className,
  "aria-label": ariaLabel = "Note",
}: {
  value: number;
  onChange: (value: number) => void;
  className?: string;
  "aria-label"?: string;
}) {
  const octave = useOctave();
  const [draft, setDraft] = useState<string | null>(null);
  const commit = (text: string) => {
    setDraft(null);
    const parsed = parseNote(text, octave);
    if (parsed !== null && parsed !== value) onChange(parsed);
  };
  return (
    <div className={cn("relative", className ?? "w-[104px]")}>
      <input
        aria-label={ariaLabel}
        value={draft ?? noteName(value, octave)}
        onChange={(event) => setDraft(event.target.value)}
        onFocus={(event) => event.target.select()}
        onBlur={(event) => commit(event.target.value)}
        onKeyDown={(event) => {
          if (event.key === "Enter") commit(event.currentTarget.value);
          else if (event.key === "Escape") setDraft(null);
          else if (event.key === "ArrowUp" || event.key === "ArrowDown") {
            event.preventDefault();
            const delta = (event.key === "ArrowUp" ? 1 : -1) * (event.shiftKey ? 12 : 1);
            setDraft(null);
            onChange(clamp(value + delta, 0, 127));
          }
        }}
        className="h-[28px] w-full rounded-md border border-line-strong bg-app pr-9 pl-2 outline-hidden hover:border-fg-faint focus:border-accent focus:ring-2 focus:ring-accent-soft"
      />
      <span className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 text-sm text-fg-faint tabular-nums">
        {value}
      </span>
    </div>
  );
}

/** Min/max editor: slider plus exact fields, with a live marker for the control's position. */
export function RangeEditor({
  value,
  onChange,
  limit,
  marker,
  offset = 0,
}: {
  value: ValueRange;
  onChange: (value: ValueRange) => void;
  /** Maximum raw value (127 or 16383). */
  limit: number;
  marker?: number | null;
  /** Subtracted for display (8192 shows pitch bend as -8192..+8191). */
  offset?: number;
}) {
  return (
    <div className="flex items-center gap-2">
      <NumberInput
        aria-label="Minimum"
        className="w-[68px]"
        min={-offset}
        max={limit - offset}
        value={value.min - offset}
        onChange={(min) => onChange({ min: min + offset, max: Math.max(value.max, min + offset) })}
      />
      <div className="min-w-0 flex-1">
        <RangeSlider
          min={0}
          max={limit}
          value={[value.min, value.max]}
          marker={marker}
          onChange={([min, max]) => onChange({ min, max })}
        />
      </div>
      <NumberInput
        aria-label="Maximum"
        className="w-[68px]"
        min={-offset}
        max={limit - offset}
        value={value.max - offset}
        onChange={(max) => onChange({ min: Math.min(value.min, max + offset), max: max + offset })}
      />
    </div>
  );
}

/** The value a log entry carries for `source`, or `null` if the entry is not about that source. */
export function sourceValue(source: Source, log: MidiLog): number | null {
  if (source.device && source.device !== log.device) return null;
  const message = log.message;
  const channelOk = (channel: number) => source.channel === null || source.channel === channel;
  const event = source.event;
  switch (event.type) {
    case "note":
      if ((message.type === "note_on" || message.type === "note_off") && channelOk(message.channel)) {
        if (message.note < event.notes.min || message.note > event.notes.max) return null;
        return message.type === "note_on" ? message.velocity : 0;
      }
      return null;
    case "control_change":
      return message.type === "control_change" && channelOk(message.channel) && message.controller === event.controller
        ? message.value
        : null;
    case "control_change14": {
      const pair = log.derived.find((d) => d.type === "control_change14");
      return pair && pair.type === "control_change14" && channelOk(pair.channel) && pair.controller === event.controller
        ? pair.value
        : null;
    }
    case "nrpn":
    case "rpn": {
      const p = log.derived.find((d) => d.type === "parameter");
      if (!p || p.type !== "parameter" || p.kind !== event.type || p.number !== event.number || !channelOk(p.channel)) {
        return null;
      }
      return event.hires ? (p.msb << 7) | p.lsb : p.msb;
    }
    case "pitch_bend":
      return message.type === "pitch_bend" && channelOk(message.channel) ? message.value : null;
    case "channel_pressure":
      return message.type === "channel_pressure" && channelOk(message.channel) ? message.pressure : null;
    case "poly_pressure":
      return message.type === "poly_pressure" &&
        channelOk(message.channel) &&
        message.note >= event.notes.min &&
        message.note <= event.notes.max
        ? message.pressure
        : null;
    case "program_change":
      return message.type === "program_change" && channelOk(message.channel) ? message.program : null;
    case "song_position":
      return message.type === "song_position" ? message.position : null;
    default:
      return null;
  }
}

/** Follows the live value of a source (for position markers and "it works" feedback). */
export function useLiveValue(source: Source): number | null {
  const [value, setValue] = useState<number | null>(null);
  const key = JSON.stringify(source);
  useEffect(() => {
    setValue(null);
    const current = JSON.parse(key) as Source;
    return onMidi((log) => {
      const next = sourceValue(current, log);
      if (next !== null) setValue(next);
    });
  }, [key]);
  return value;
}
