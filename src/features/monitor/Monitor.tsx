import { ArrowRight, Ban, CornerDownRight, Eraser, Pause, Play, TriangleAlert, X } from "lucide-react";
import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import type { MidiLog } from "@/bindings/MidiLog";
import type { Outcome } from "@/bindings/Outcome";
import { IconButton } from "@/components/ui/button";
import { NumberInput, Segmented, Select, Switch } from "@/components/ui/inputs";
import { deviceLabel, sourceTitle } from "@/lib/describe";
import { bytesToHex, encodeMessage, isBlackKey, noteName, viewMessage } from "@/lib/midi";
import { clearLog, onFired, onMidi, setLogFrozen, useLog, useLogFrozen, type LogEntry } from "@/lib/monitor";
import { api, profileOfMapping, useApp, useOctave } from "@/lib/store";
import { cn } from "@/lib/utils";

function formatTime(us: number): string {
  const ms = Math.floor(us / 1000);
  const minutes = Math.floor(ms / 60000);
  const seconds = Math.floor((ms % 60000) / 1000);
  return `${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}.${String(ms % 1000).padStart(3, "0")}`;
}

const SKIP_TEXT: Record<string, string> = {
  paused: "not sent — output is paused",
  window_focused: "not sent — this window is in front",
  learning: "not sent — learning",
  rate_limited: "held back by the rate limit",
  condition_failed: "condition not met",
};

export function outcomeText(outcome: Outcome): string | null {
  if (outcome.type === "sent") return null;
  if (outcome.type === "failed") return `failed: ${outcome.error}`;
  return SKIP_TEXT[outcome.reason] ?? outcome.reason;
}

function mappingLabel(id: string): string {
  const config = useApp.getState().config;
  const owner = profileOfMapping(config, id);
  const mapping = owner?.mappings.find((m) => m.id === id);
  if (!mapping) return "a deleted mapping";
  return mapping.name || sourceTitle(mapping.source.event, config.settings.middle_c_octave);
}

const KIND_COLOR: Record<string, string> = {
  note: "text-midi",
  cc: "text-midi",
  program: "text-midi",
  bend: "text-midi",
  pressure: "text-midi",
  sysex: "text-accent",
  clock: "text-fg-faint",
  transport: "text-accent",
  system: "text-fg-muted",
};

const LogRow = memo(function LogRow({ entry, octave }: { entry: LogEntry; octave: number }) {
  const selectMapping = useApp((state) => state.selectMapping);
  const config = useApp((state) => state.config);

  if (entry.kind === "error") {
    return (
      <div className="flex h-[22px] items-center gap-2 px-3 text-danger">
        <span className="w-[74px] shrink-0 font-mono text-sm text-fg-faint">{formatTime(entry.time_us)}</span>
        <TriangleAlert size={12} className="shrink-0" />
        <span className="selectable truncate">{entry.message}</span>
      </div>
    );
  }

  if (entry.kind === "fired") {
    const note = outcomeText(entry.outcome);
    const sent = entry.outcome.type === "sent";
    return (
      <div className="flex h-[22px] items-center gap-2 px-3">
        <span className="w-[74px] shrink-0 font-mono text-sm text-fg-faint">{formatTime(entry.time_us)}</span>
        <span className="w-[130px] shrink-0" />
        {sent ? (
          <CornerDownRight size={12} className="shrink-0 text-key" />
        ) : (
          <Ban size={12} className={cn("shrink-0", entry.outcome.type === "failed" ? "text-danger" : "text-fg-faint")} />
        )}
        <span className={cn("truncate font-medium", sent ? "text-key" : "text-fg-faint line-through decoration-fg-faint/50")}>
          {entry.description}
        </span>
        {note && (
          <span className={cn("truncate text-sm", entry.outcome.type === "failed" ? "text-danger" : "text-fg-faint")}>{note}</span>
        )}
        {entry.mapping_id && (
          <button
            type="button"
            className="ml-auto shrink-0 truncate text-sm text-fg-faint underline-offset-2 hover:text-fg hover:underline"
            onClick={() => selectMapping(entry.mapping_id)}
          >
            {mappingLabel(entry.mapping_id)}
          </button>
        )}
      </div>
    );
  }

  const view = viewMessage(entry.message, octave);
  const derived = entry.derived
    .map((d) =>
      d.type === "parameter"
        ? `${d.kind.toUpperCase()} ${d.number} = ${(d.msb << 7) | d.lsb} (${d.change.replace("_", " ")})`
        : `14-bit CC ${d.controller} = ${d.value}`,
    )
    .join(" · ");
  return (
    <div className={cn("flex h-[22px] items-center gap-2 px-3", entry.mappings.length === 0 && "opacity-70")}>
      <span className="w-[74px] shrink-0 font-mono text-sm text-fg-faint">{formatTime(entry.time_us)}</span>
      <span className="w-[130px] shrink-0 truncate text-sm text-fg-muted">{deviceLabel(entry.device, config)}</span>
      <span className={cn("w-[84px] shrink-0 truncate text-sm", KIND_COLOR[view.kind])}>{view.label}</span>
      <span className="w-[42px] shrink-0 text-sm text-fg-faint">{view.channel !== null ? `ch ${view.channel}` : ""}</span>
      <span className="shrink-0 font-medium">{view.subject}</span>
      <span className="shrink-0 font-mono text-sm text-fg-muted tabular-nums">{view.detail}</span>
      {derived && <span className="truncate text-sm text-accent">{derived}</span>}
      <span className="selectable ml-auto shrink-0 pl-3 font-mono text-sm text-fg-faint">
        {bytesToHex(encodeMessage(entry.message).slice(0, 8))}
      </span>
    </div>
  );
});

type LogFilter = "all" | "matched" | "actions";

function LogView() {
  const entries = useLog();
  const frozen = useLogFrozen();
  const octave = useOctave();
  const [filter, setFilter] = useState<LogFilter>("all");
  const [showNoise, setShowNoise] = useState(false);
  const scroller = useRef<HTMLDivElement>(null);
  const pinned = useRef(true);

  const visible = useMemo(
    () =>
      entries.filter((entry) => {
        if (entry.kind === "error") return true;
        if (entry.kind === "fired") return filter !== "matched" || true;
        if (!showNoise && (entry.message.type === "clock" || entry.message.type === "active_sensing")) return false;
        if (filter === "actions") return false;
        if (filter === "matched") return entry.mappings.length > 0;
        return true;
      }),
    [entries, filter, showNoise],
  );

  useLayoutEffect(() => {
    const element = scroller.current;
    if (element && pinned.current) element.scrollTop = element.scrollHeight;
  }, [visible]);

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="flex h-[34px] shrink-0 items-center gap-2 border-b px-3">
        <Segmented<LogFilter>
          size="sm"
          value={filter}
          options={[
            { value: "all", label: "Everything" },
            { value: "matched", label: "Mapped only", title: "Only MIDI that a mapping reacts to" },
            { value: "actions", label: "Output only", title: "Only what was sent to the computer" },
          ]}
          onChange={setFilter}
        />
        <label className="flex items-center gap-1.5 text-sm text-fg-muted">
          <Switch aria-label="Show clock and active sensing" checked={showNoise} onChange={setShowNoise} />
          Clock &amp; active sensing
        </label>
        <div className="flex-1" />
        <IconButton label={frozen ? "Resume the log" : "Freeze the log"} size="sm" active={frozen} onClick={() => setLogFrozen(!frozen)}>
          {frozen ? <Play size={13} /> : <Pause size={13} />}
        </IconButton>
        <IconButton label="Clear" size="sm" onClick={clearLog}>
          <Eraser size={13} />
        </IconButton>
      </div>
      <div
        ref={scroller}
        onScroll={(e) => {
          const el = e.currentTarget;
          pinned.current = el.scrollHeight - el.scrollTop - el.clientHeight < 30;
        }}
        className="min-h-0 flex-1 overflow-y-auto py-1"
      >
        {visible.length === 0 ? (
          <div className="flex h-full items-center justify-center text-fg-faint">
            {entries.length === 0
              ? "Waiting for MIDI… play a note or move a control and it shows up here."
              : "Nothing matches this filter."}
          </div>
        ) : (
          visible.map((entry) => <LogRow key={entry.id} entry={entry} octave={octave} />)
        )}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Virtual controller: send MIDI without hardware.
// ---------------------------------------------------------------------------------------------

const VIRTUAL_DEVICE = "Virtual controller";

function VirtualController() {
  const octave = useOctave();
  const [channel, setChannel] = useState(1);
  const [base, setBase] = useState(48);
  const [velocity, setVelocity] = useState(100);
  const [controller, setController] = useState(20);
  const [ccValue, setCcValue] = useState(64);
  const [bend, setBend] = useState(8192);
  const [program, setProgram] = useState(0);
  const held = useRef(new Set<number>());

  const send = async (bytes: number[]) => {
    const backend = await api();
    void backend.sendMidi(VIRTUAL_DEVICE, bytes);
  };
  const status = (kind: number) => kind | (channel - 1);
  const noteOn = (note: number) => {
    if (held.current.has(note)) return;
    held.current.add(note);
    void send([status(0x90), note, velocity]);
  };
  const noteOff = (note: number) => {
    if (!held.current.delete(note)) return;
    void send([status(0x80), note, 0]);
  };

  useEffect(() => {
    const notes = held.current;
    const releaseAll = () => Array.from(notes).forEach(noteOff);
    window.addEventListener("pointerup", releaseAll);
    window.addEventListener("blur", releaseAll);
    return () => {
      window.removeEventListener("pointerup", releaseAll);
      window.removeEventListener("blur", releaseAll);
      releaseAll();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [channel]);

  const notes = Array.from({ length: 25 }, (_, i) => base + i).filter((n) => n <= 127);
  const whites = notes.filter((n) => !isBlackKey(n));
  const whiteWidth = 100 / whites.length;

  return (
    <div className="flex min-h-0 flex-1 gap-5 overflow-auto px-4 py-3">
      <div className="flex min-w-[420px] flex-1 flex-col gap-2">
        <div className="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
          <span>Channel</span>
          <NumberInput aria-label="Channel" className="w-[56px]" min={1} max={16} value={channel} onChange={setChannel} />
          <span className="pl-2">Velocity</span>
          <NumberInput aria-label="Velocity" className="w-[60px]" min={1} max={127} value={velocity} onChange={setVelocity} />
          <span className="pl-2">Octave</span>
          <Segmented
            size="sm"
            value={String(base)}
            options={[24, 36, 48, 60, 72].map((n) => ({ value: String(n), label: noteName(n, octave) }))}
            onChange={(value) => setBase(Number(value))}
          />
        </div>
        <div className="relative h-[92px] select-none">
          {whites.map((note, index) => (
            <button
              key={note}
              type="button"
              aria-label={noteName(note, octave)}
              onPointerDown={(e) => {
                e.currentTarget.releasePointerCapture?.(e.pointerId);
                noteOn(note);
              }}
              onPointerEnter={(e) => e.buttons === 1 && noteOn(note)}
              onPointerLeave={() => noteOff(note)}
              onPointerUp={() => noteOff(note)}
              style={{ left: `${index * whiteWidth}%`, width: `${whiteWidth}%` }}
              className="absolute top-0 bottom-0 flex items-end justify-center rounded-b-md border border-line-strong bg-[#f4f5f8] pb-1 text-2xs text-[#666] active:bg-midi"
            >
              {note % 12 === 0 ? noteName(note, octave) : ""}
            </button>
          ))}
          {notes
            .filter((n) => isBlackKey(n))
            .map((note) => {
              const leftWhite = whites.filter((w) => w < note).length;
              return (
                <button
                  key={note}
                  type="button"
                  aria-label={noteName(note, octave)}
                  onPointerDown={(e) => {
                    e.currentTarget.releasePointerCapture?.(e.pointerId);
                    noteOn(note);
                  }}
                  onPointerEnter={(e) => e.buttons === 1 && noteOn(note)}
                  onPointerLeave={() => noteOff(note)}
                  onPointerUp={() => noteOff(note)}
                  style={{ left: `${leftWhite * whiteWidth - whiteWidth * 0.3}%`, width: `${whiteWidth * 0.6}%` }}
                  className="absolute top-0 z-10 h-[58%] rounded-b border border-black bg-[#22242c] active:bg-midi"
                />
              );
            })}
        </div>
      </div>

      <div className="flex w-[330px] shrink-0 flex-col gap-2.5 text-sm text-fg-muted">
        <div className="flex items-center gap-2">
          <span className="w-[42px]">CC</span>
          <NumberInput aria-label="Controller" className="w-[60px]" min={0} max={127} value={controller} onChange={setController} />
          <input
            type="range"
            aria-label="Controller value"
            min={0}
            max={127}
            value={ccValue}
            onChange={(e) => {
              const value = Number(e.target.value);
              setCcValue(value);
              void send([status(0xb0), controller, value]);
            }}
            className="min-w-0 flex-1 accent-(--midi)"
          />
          <span className="w-[28px] text-right font-mono tabular-nums">{ccValue}</span>
        </div>
        <div className="flex items-center gap-2">
          <span className="w-[42px]">Encoder</span>
          <button type="button" className="h-[24px] rounded-md border border-line-strong px-2 hover:bg-hover" onClick={() => send([status(0xb0), controller, 127])}>
            ◀ −1
          </button>
          <button type="button" className="h-[24px] rounded-md border border-line-strong px-2 hover:bg-hover" onClick={() => send([status(0xb0), controller, 1])}>
            +1 ▶
          </button>
          <span className="text-fg-faint">relative ticks on the same CC</span>
        </div>
        <div className="flex items-center gap-2">
          <span className="w-[42px]">Bend</span>
          <input
            type="range"
            aria-label="Pitch bend"
            min={0}
            max={16383}
            value={bend}
            onChange={(e) => {
              const value = Number(e.target.value);
              setBend(value);
              void send([status(0xe0), value & 0x7f, value >> 7]);
            }}
            onPointerUp={() => {
              setBend(8192);
              void send([status(0xe0), 0, 64]);
            }}
            className="min-w-0 flex-1 accent-(--midi)"
          />
          <span className="w-[44px] text-right font-mono tabular-nums">{bend - 8192}</span>
        </div>
        <div className="flex items-center gap-2">
          <span className="w-[42px]">Other</span>
          <NumberInput aria-label="Program" className="w-[60px]" min={0} max={127} value={program} onChange={setProgram} />
          <button type="button" className="h-[24px] rounded-md border border-line-strong px-2 hover:bg-hover" onClick={() => send([status(0xc0), program])}>
            Program
          </button>
          <Select
            aria-label="Transport"
            size="sm"
            className="w-[100px]"
            value=""
            placeholder="Transport"
            options={[
              { value: "fa", label: "Start" },
              { value: "fc", label: "Stop" },
              { value: "fb", label: "Continue" },
            ]}
            onChange={(value) => void send([parseInt(value, 16)])}
          />
        </div>
      </div>
    </div>
  );
}

export function MonitorPanel() {
  const setMonitorOpen = useApp((state) => state.setMonitorOpen);
  const tab = useApp((state) => state.monitorTab);
  const setTab = useApp((state) => state.setMonitorTab);
  return (
    <section aria-label="Monitor" className="flex h-[250px] shrink-0 flex-col border-t bg-surface">
      <div className="flex h-[34px] shrink-0 items-center gap-2 border-b pr-2 pl-3">
        <Segmented
          size="sm"
          value={tab}
          options={[
            { value: "log", label: "Live log" },
            { value: "virtual", label: "Virtual controller", title: "Send MIDI without any hardware" },
          ]}
          onChange={setTab}
        />
        <span className="min-w-0 flex-1 truncate text-sm text-fg-faint">
          {tab === "log"
            ? "Everything that comes in, and everything that goes out."
            : "Click the keys or move the sliders: it is treated exactly like a real device."}
        </span>
        <IconButton label="Close monitor" size="sm" onClick={() => setMonitorOpen(false)}>
          <X size={14} />
        </IconButton>
      </div>
      {tab === "log" ? <LogView /> : <VirtualController />}
    </section>
  );
}

/** One-line live preview for the status bar: the last message and what it caused. */
export function LastEvent() {
  const octave = useOctave();
  const config = useApp((state) => state.config);
  const [midi, setMidi] = useState<MidiLog | null>(null);
  const [fired, setFired] = useState<{ text: string; sent: boolean; note: string | null } | null>(null);
  const firedAt = useRef(0);

  useEffect(() => {
    const offMidi = onMidi((log) => {
      if (log.message.type === "clock" || log.message.type === "active_sensing") return;
      setMidi(log);
      if (performance.now() - firedAt.current > 250) setFired(null);
    });
    const offFired = onFired((log) => {
      firedAt.current = performance.now();
      setFired({ text: log.description, sent: log.outcome.type === "sent", note: outcomeText(log.outcome) });
    });
    return () => {
      offMidi();
      offFired();
    };
  }, []);

  if (!midi) return <span className="truncate text-fg-faint">No MIDI received yet</span>;
  const view = viewMessage(midi.message, octave);
  return (
    <span className="flex min-w-0 items-center gap-1.5">
      <span className="shrink-0 text-fg-faint">{deviceLabel(midi.device, config)}</span>
      <span className="shrink-0 font-medium text-midi">
        {view.label === "CC" ? "" : `${view.label} `}
        {view.subject}
      </span>
      <span className="shrink-0 font-mono text-fg-muted tabular-nums">{view.detail}</span>
      {view.channel !== null && <span className="shrink-0 text-fg-faint">ch {view.channel}</span>}
      {fired && (
        <>
          <ArrowRight size={12} className="shrink-0 text-fg-faint" />
          <span className={cn("truncate font-medium", fired.sent ? "text-key" : "text-fg-faint")}>{fired.text}</span>
          {fired.note && <span className="truncate text-fg-faint">({fired.note})</span>}
        </>
      )}
    </span>
  );
}
