import { CircleDot, Radio, Square, X } from "lucide-react";
import { useState } from "react";

import type { Source } from "@/bindings/Source";
import type { SourceEvent } from "@/bindings/SourceEvent";
import type { SystemMessage } from "@/bindings/SystemMessage";
import type { ValueEncoding } from "@/bindings/ValueEncoding";
import { Button } from "@/components/ui/button";
import { NumberInput, Segmented, Select, Switch, TextInput } from "@/components/ui/inputs";
import { Collapsible, Field } from "@/components/ui/misc";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/overlays";
import { sourceNickname, sourceQualifiers, sourceTitle } from "@/lib/describe";
import { useLearn, type LearnResult } from "@/lib/learn";
import { ccName, normalizeSysExPattern, noteName, SYSEX_PRESETS } from "@/lib/midi";
import {
  defaultSourceEvent,
  FULL7,
  isFullRange,
  rangeLimit,
  SOURCE_TYPES,
  sourceShape,
  sourceTypeInfo,
  UPPER7,
  type SourceType,
} from "@/lib/model";
import { useApp, useOctave } from "@/lib/store";
import { cn } from "@/lib/utils";

import { ChannelSelect, DeviceSelect, NoteInput, parseNote, RangeEditor, useLiveValue } from "./fields";
import { SourceIcon } from "../mappings/tokens";

const TYPE_OPTIONS = SOURCE_TYPES.map((info) => ({ value: info.type, label: info.label, hint: info.hint }));

const SYSTEM_OPTIONS: { value: SystemMessage; label: string }[] = [
  { value: "start", label: "Start" },
  { value: "stop", label: "Stop" },
  { value: "continue", label: "Continue" },
  { value: "tune_request", label: "Tune Request" },
  { value: "reset", label: "System Reset" },
  { value: "active_sensing", label: "Active Sensing" },
];

const ENCODING_OPTIONS: { value: ValueEncoding; label: string; hint: string }[] = [
  { value: "relative_twos_complement", label: "Two's complement", hint: "1 = up, 127 = down (most common)" },
  { value: "relative_binary_offset", label: "Binary offset", hint: "65 = up, 63 = down" },
  { value: "relative_sign_magnitude", label: "Sign-magnitude", hint: "1 = up, 65 = down" },
];

export function SourceEditor({
  source,
  onChange,
  onLearned,
  autoLearn = false,
}: {
  source: Source;
  onChange: (source: Source, coalesceKey?: string) => void;
  /** A learn session finished; the parent applies the result (and may adapt the behavior). */
  onLearned: (result: LearnResult) => void;
  autoLearn?: boolean;
}) {
  const [learning, setLearning] = useState(autoLearn);
  const multipleDevices = useApp((state) => state.ports.filter((p) => p.state === "connected").length > 1);
  const config = useApp((state) => state.config);
  const octave = useOctave();
  const live = useLiveValue(source);

  const learn = useLearn(learning, { pinDevice: multipleDevices, chord: source.event.type === "chord" }, (result) => {
    setLearning(false);
    onLearned(result);
  });

  const event = source.event;
  const info = sourceTypeInfo(event.type);
  const setEvent = (next: SourceEvent, key?: string) => onChange({ ...source, event: next }, key);
  const limit = rangeLimit(event);
  const nickname = sourceNickname(event);
  const qualifiers = sourceQualifiers(source, config);

  return (
    <div className="flex flex-col gap-3">
      {/* Summary / learn card */}
      <div
        className={cn(
          "flex min-h-[54px] items-center gap-3 rounded-lg border px-3 py-2 transition-colors",
          learning
            ? "animate-listen border-midi bg-midi-soft [--ring-color:var(--midi-line)]"
            : "border-midi-line bg-midi-soft",
        )}
      >
        {learning ? (
          <>
            <Radio size={18} className="shrink-0 animate-pulse text-midi" />
            <div className="min-w-0 flex-1">
              {learn.phase === "listening" && (
                <>
                  <div className="font-medium text-fg">Listening…</div>
                  <div className="text-sm text-fg-muted">
                    {event.type === "chord"
                      ? "Hold all the notes of the chord together, then let go."
                      : "Press a key or pad, turn a knob, move a fader."}
                  </div>
                </>
              )}
              {learn.phase === "detecting" && (
                <>
                  <div className="font-medium text-fg">
                    {sourceTitle(learn.preview.source.event, octave)}{" "}
                    <span className="font-normal text-fg-muted">— looks like {learn.preview.what}</span>
                  </div>
                  <div className="text-sm text-fg-muted">{learn.hint}</div>
                </>
              )}
            </div>
            <Button size="sm" variant="ghost" icon={<Square size={11} />} onClick={() => setLearning(false)}>
              Stop
            </Button>
          </>
        ) : (
          <>
            <SourceIcon event={event} className="shrink-0 text-midi" size={18} />
            <div className="min-w-0 flex-1">
              <div className="flex items-baseline gap-2">
                <span className="text-lg leading-tight font-semibold text-fg">{sourceTitle(event, octave)}</span>
                {nickname && <span className="truncate text-sm text-fg-muted">{nickname}</span>}
              </div>
              <div className="truncate text-sm text-fg-muted">
                {qualifiers.length ? qualifiers.join(" · ") : source.channel === null && info.channel ? "any channel" : info.hint}
              </div>
            </div>
            {live !== null && (
              <div className="shrink-0 text-right">
                <div className="font-mono text-base leading-tight text-midi tabular-nums">{live}</div>
                <div className="text-2xs tracking-wide text-fg-faint uppercase">live</div>
              </div>
            )}
            <Button variant="midi" icon={<CircleDot size={14} />} onClick={() => setLearning(true)}>
              Learn
            </Button>
          </>
        )}
      </div>

      <Field label="Message">
        <div className="flex gap-2">
          <Select
            aria-label="Message type"
            className="min-w-0 flex-1"
            value={event.type}
            options={TYPE_OPTIONS}
            onChange={(type: SourceType) =>
              onChange({
                ...source,
                channel: sourceTypeInfo(type).channel ? source.channel : null,
                event: defaultSourceEvent(type),
              })
            }
          />
          {info.channel && (
            <ChannelSelect value={source.channel} onChange={(channel) => onChange({ ...source, channel })} />
          )}
        </div>
      </Field>

      {event.type === "note" && (
        <Field
          label="Note"
          hint={
            event.notes.min !== event.notes.max
              ? "Every note in the range triggers this mapping on its own."
              : undefined
          }
        >
          <div className="flex items-center gap-2">
            <NoteInput
              value={event.notes.min}
              onChange={(note) =>
                setEvent(
                  {
                    ...event,
                    notes:
                      event.notes.min === event.notes.max
                        ? { min: note, max: note }
                        : { min: note, max: Math.max(note, event.notes.max) },
                  },
                  "note",
                )
              }
            />
            {event.notes.min !== event.notes.max && (
              <>
                <span className="text-fg-faint">to</span>
                <NoteInput
                  aria-label="Last note"
                  value={event.notes.max}
                  onChange={(note) =>
                    setEvent({ ...event, notes: { min: Math.min(note, event.notes.min), max: note } }, "note")
                  }
                />
              </>
            )}
            <button
              type="button"
              className="text-sm text-fg-muted underline-offset-2 hover:text-fg hover:underline"
              onClick={() =>
                setEvent({
                  ...event,
                  notes:
                    event.notes.min === event.notes.max
                      ? { min: event.notes.min, max: Math.min(127, event.notes.min + 12) }
                      : { min: event.notes.min, max: event.notes.min },
                })
              }
            >
              {event.notes.min === event.notes.max ? "use a range" : "single note"}
            </button>
          </div>
        </Field>
      )}

      {event.type === "chord" && (
        <Field
          label="Notes"
          align="start"
          hint="Fires when all of these are held together. Press Learn and play the chord, or add notes one by one."
        >
          <div className="flex flex-wrap items-center gap-1.5">
            {event.notes.map((note) => (
              <span
                key={note}
                className="inline-flex h-[26px] items-center gap-0.5 rounded-md border border-midi-line bg-midi-soft pr-0.5 pl-2 font-medium text-midi"
              >
                {noteName(note, octave)}
                <button
                  type="button"
                  aria-label={`Remove ${noteName(note, octave)}`}
                  onClick={() => setEvent({ ...event, notes: event.notes.filter((n) => n !== note) })}
                  className="inline-flex size-[20px] items-center justify-center rounded text-midi/70 hover:bg-midi/20 hover:text-midi"
                >
                  <X size={12} />
                </button>
              </span>
            ))}
            <AddNote
              onAdd={(note) =>
                setEvent({ ...event, notes: Array.from(new Set([...event.notes, note])).sort((a, b) => a - b) })
              }
            />
          </div>
        </Field>
      )}

      {event.type === "control_change" && (
        <>
          <Field label="Controller" hint={ccName(event.controller) || undefined}>
            <NumberInput
              aria-label="Controller number"
              min={0}
              max={127}
              value={event.controller}
              onChange={(controller) => setEvent({ ...event, controller }, "controller")}
            />
          </Field>
          <Field
            label="It is a"
            align="start"
            hint={
              sourceShape(event) === "continuous"
                ? "Reacts to every movement."
                : sourceShape(event) === "button"
                  ? "Counts as pressed while its value is inside the range below."
                  : "Sends relative ticks instead of a position."
            }
          >
            <Segmented
              className="w-full"
              value={sourceShape(event) as "continuous" | "button" | "relative"}
              options={[
                { value: "continuous", label: "Knob / fader" },
                { value: "button", label: "Button / pedal" },
                { value: "relative", label: "Endless encoder" },
              ]}
              onChange={(shape) =>
                setEvent({
                  ...event,
                  encoding: shape === "relative" ? "relative_twos_complement" : "absolute",
                  range: shape === "button" ? { ...UPPER7 } : { ...FULL7 },
                })
              }
            />
          </Field>
          {sourceShape(event) === "button" && (
            <Field label="Pressed when">
              <RangeEditor
                limit={127}
                value={event.range}
                marker={live}
                onChange={(range) => setEvent({ ...event, range }, "range")}
              />
            </Field>
          )}
          {sourceShape(event) === "relative" && (
            <Field label="Encoding" hint="If turning does nothing or jumps, try another encoding — or use Learn and turn the encoder both ways.">
              <Select
                aria-label="Relative encoding"
                value={event.encoding}
                options={ENCODING_OPTIONS}
                onChange={(encoding) => setEvent({ ...event, encoding })}
              />
            </Field>
          )}
        </>
      )}

      {event.type === "control_change14" && (
        <Field label="Controller" hint={`MSB on CC ${event.controller}, LSB on CC ${event.controller + 32}.`}>
          <NumberInput
            aria-label="MSB controller number"
            min={0}
            max={31}
            value={event.controller}
            onChange={(controller) => setEvent({ ...event, controller }, "controller")}
          />
        </Field>
      )}

      {(event.type === "nrpn" || event.type === "rpn") && (
        <>
          <Field
            label="Parameter"
            hint={`MSB ${event.number >> 7} · LSB ${event.number & 0x7f}  (CC ${event.type === "nrpn" ? "99 / 98" : "101 / 100"})`}
          >
            <NumberInput
              aria-label="Parameter number"
              className="w-[96px]"
              min={0}
              max={16383}
              value={event.number}
              onChange={(number) => setEvent({ ...event, number }, "number")}
            />
          </Field>
          <Field label="14-bit data" hint="On: wait for the data LSB (CC 38) and use the full 0–16383 value.">
            <Switch
              aria-label="14-bit data"
              checked={event.hires}
              onChange={(hires) =>
                setEvent({ ...event, hires, range: { min: 0, max: hires ? 16383 : 127 } })
              }
            />
          </Field>
        </>
      )}

      {event.type === "program_change" && (
        <Field label="Program">
          <div className="flex items-center gap-2">
            {event.programs.min === 0 && event.programs.max === 127 ? (
              <span className="text-fg-muted">Any program</span>
            ) : (
              <NumberInput
                aria-label="Program number"
                min={0}
                max={127}
                value={event.programs.min}
                onChange={(program) => setEvent({ ...event, programs: { min: program, max: program } }, "program")}
              />
            )}
            <button
              type="button"
              className="text-sm text-fg-muted underline-offset-2 hover:text-fg hover:underline"
              onClick={() =>
                setEvent({
                  ...event,
                  programs: event.programs.min === 0 && event.programs.max === 127 ? { min: 0, max: 0 } : { ...FULL7 },
                })
              }
            >
              {event.programs.min === 0 && event.programs.max === 127 ? "pick one" : "any program"}
            </button>
          </div>
        </Field>
      )}

      {event.type === "poly_pressure" && (
        <Field label="Note">
          <NoteInput
            value={event.notes.min}
            onChange={(note) => setEvent({ ...event, notes: { min: note, max: note } }, "note")}
          />
        </Field>
      )}

      {event.type === "sys_ex" && <SysExField pattern={event.pattern} onChange={(pattern) => setEvent({ ...event, pattern }, "pattern")} />}

      {event.type === "system" && (
        <Field label="Event">
          <Select
            aria-label="System message"
            value={event.message}
            options={SYSTEM_OPTIONS}
            onChange={(message) => setEvent({ ...event, message })}
          />
        </Field>
      )}

      {event.type === "clock" && (
        <Field label="Every" hint="MIDI clock sends 24 ticks per quarter note. The count restarts on Start.">
          <div className="flex items-center gap-2">
            <NumberInput
              aria-label="Clock ticks"
              suffix="ticks"
              className="w-[96px]"
              min={1}
              max={9600}
              value={event.divisor}
              onChange={(divisor) => setEvent({ ...event, divisor }, "divisor")}
            />
            <Segmented
              size="sm"
              value={String(event.divisor)}
              options={[
                { value: "6", label: "1/16" },
                { value: "12", label: "1/8" },
                { value: "24", label: "Beat" },
                { value: "96", label: "Bar" },
              ]}
              onChange={(value) => setEvent({ ...event, divisor: Number(value) })}
            />
          </div>
        </Field>
      )}

      {event.type === "song_select" && (
        <Field label="Song">
          <NumberInput
            aria-label="Song number"
            min={0}
            max={127}
            value={event.songs.min}
            onChange={(song) => setEvent({ ...event, songs: { min: song, max: song } }, "song")}
          />
        </Field>
      )}

      <Collapsible
        title="More"
        summary={[
          event.type === "note" && (event.velocity.min > 1 || event.velocity.max < 127) && "velocity range",
          event.type === "chord" && event.window_ms > 0 && `within ${event.window_ms} ms`,
          source.hysteresis > 0 && `slack ${source.hysteresis}`,
          source.device && "one device",
        ]
          .filter(Boolean)
          .join(" · ")}
      >
        <div className="flex flex-col gap-3">
          {event.type === "note" && (
            <Field
              label="Velocity"
              align="start"
              hint="Only react to hits in this range. Make one mapping per zone to give soft and hard hits different jobs."
            >
              <RangeEditor
                limit={127}
                value={event.velocity}
                marker={live || null}
                onChange={(velocity) =>
                  setEvent({ ...event, velocity: { min: Math.max(1, velocity.min), max: Math.max(1, velocity.max) } }, "velocity")
                }
              />
            </Field>
          )}
          {"range" in event && event.type !== "control_change" && limit !== null && (
            <Field
              label="Active range"
              align="start"
              hint="Full range reacts to every movement. Narrow it and the control acts like a button that is pressed while the value is inside."
            >
              <RangeEditor
                limit={limit}
                offset={event.type === "pitch_bend" ? 8192 : 0}
                value={event.range}
                marker={live}
                onChange={(range) => setEvent({ ...event, range } as SourceEvent, "range")}
              />
            </Field>
          )}
          {event.type === "chord" && (
            <Field
              label="Struck within"
              hint="0 = no limit: holding some notes and adding the rest later counts. Set a short time (try 60 ms) so only notes hit together count."
            >
              <NumberInput
                aria-label="Chord window"
                suffix="ms"
                className="w-[92px]"
                min={0}
                max={10000}
                value={event.window_ms}
                onChange={(window_ms) => setEvent({ ...event, window_ms }, "window")}
              />
            </Field>
          )}
          {limit !== null && "range" in event && !isFullRange(event) && sourceShape(event) === "button" && (
            <Field
              label="Slack"
              hint="How far past the edge of the range the value must go before it counts as released. Stops a jittery pedal or fader from flickering on and off."
            >
              <NumberInput
                aria-label="Hysteresis"
                className="w-[76px]"
                min={0}
                max={Math.floor(limit / 2)}
                value={source.hysteresis}
                onChange={(hysteresis) => onChange({ ...source, hysteresis }, "hysteresis")}
              />
            </Field>
          )}
          <Field label="Device" hint="Limit this mapping to one controller.">
            <DeviceSelect value={source.device} onChange={(device) => onChange({ ...source, device })} />
          </Field>
        </div>
      </Collapsible>
    </div>
  );
}

/** A small "type a note and press Enter" field for building chords by hand. */
function AddNote({ onAdd }: { onAdd: (note: number) => void }) {
  const octave = useOctave();
  const [text, setText] = useState("");
  const [bad, setBad] = useState(false);
  const commit = () => {
    if (!text.trim()) return;
    const note = parseNote(text, octave);
    if (note === null) {
      setBad(true);
      return;
    }
    onAdd(note);
    setText("");
  };
  return (
    <input
      aria-label="Add a note"
      placeholder="+ note"
      value={text}
      onChange={(event) => {
        setText(event.target.value);
        setBad(false);
      }}
      onBlur={commit}
      onKeyDown={(event) => event.key === "Enter" && commit()}
      className={cn(
        "h-[26px] w-[72px] rounded-md border bg-app px-2 outline-hidden placeholder:text-fg-faint focus:border-accent",
        bad ? "border-danger" : "border-line-strong hover:border-fg-faint",
      )}
    />
  );
}

function SysExField({ pattern, onChange }: { pattern: string; onChange: (pattern: string) => void }) {
  const { error } = normalizeSysExPattern(pattern);
  return (
    <Field
      label="Pattern"
      align="start"
      hint={
        error ? (
          <span className="text-danger">{error}</span>
        ) : (
          <>
            Hex bytes. <code className="font-mono">??</code> matches any one byte,{" "}
            <code className="font-mono">**</code> any run of bytes.
          </>
        )
      }
    >
      <div className="flex gap-1">
        <TextInput
          mono
          aria-label="SysEx pattern"
          invalid={!!error}
          value={pattern}
          onChange={(value) => onChange(value.toUpperCase())}
          onBlur={() => {
            const normalized = normalizeSysExPattern(pattern);
            if (!normalized.error && normalized.pattern !== pattern) onChange(normalized.pattern);
          }}
          placeholder="F0 7F ?? 06 02 F7"
        />
        <Menu>
          <MenuTrigger asChild>
            <Button>Presets</Button>
          </MenuTrigger>
          <MenuContent>
            {SYSEX_PRESETS.map((preset) => (
              <MenuItem key={preset.label} onSelect={() => onChange(preset.pattern)}>
                {preset.label}
              </MenuItem>
            ))}
          </MenuContent>
        </Menu>
      </div>
    </Field>
  );
}
