// MIDI learn: watches the incoming stream and works out *what kind of control* the user touched,
// not just which message arrived — a button, an absolute knob/fader, an endless encoder (and which
// of the three relative encodings it speaks), an NRPN/RPN parameter, a 14-bit pair...

import { useEffect, useRef, useState } from "react";

import type { MidiLog } from "@/bindings/MidiLog";
import type { Source } from "@/bindings/Source";
import type { ValueEncoding } from "@/bindings/ValueEncoding";

import { bytesToHex, ccName } from "./midi";
import { FULL14, FULL7, sourceShape, type SourceShape } from "./model";
import { onMidi } from "./monitor";
import { api } from "./store";

export interface LearnResult {
  source: Source;
  shape: SourceShape;
  /** What the control was recognized as, in a few words: "a pad", "a knob", "an endless encoder". */
  what: string;
}

export type LearnState =
  | { phase: "listening" }
  | { phase: "detecting"; preview: LearnResult; hint: string }
  | { phase: "done"; result: LearnResult };

/** How long to keep watching a control change before deciding what kind of control sends it. */
const SETTLE_MS = 380;
const MAX_DETECT_MS = 1600;
const ENOUGH_SAMPLES = 8;

interface CcCandidate {
  device: string;
  channel: number;
  controller: number;
  values: number[];
  /** Controllers seen interleaved with ours (for 14-bit pair detection). */
  partnerHits: number;
  startedAt: number;
}

interface ParamCandidate {
  device: string;
  channel: number;
  kind: "nrpn" | "rpn";
  number: number;
  hires: boolean;
  startedAt: number;
}

export interface LearnOptions {
  /** Pin the learned source to the device it came from (worth doing when several are connected). */
  pinDevice: boolean;
}

export class Learner {
  private cc: CcCandidate | null = null;
  private param: ParamCandidate | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private finished = false;

  constructor(
    private readonly options: LearnOptions,
    private readonly emit: (state: LearnState) => void,
  ) {}

  dispose(): void {
    if (this.timer) clearTimeout(this.timer);
    this.finished = true;
  }

  private device(name: string): string | null {
    return this.options.pinDevice ? name : null;
  }

  private done(result: LearnResult): void {
    if (this.finished) return;
    this.finished = true;
    if (this.timer) clearTimeout(this.timer);
    this.emit({ phase: "done", result });
  }

  private settle(run: () => void, delay = SETTLE_MS): void {
    if (this.timer) clearTimeout(this.timer);
    this.timer = setTimeout(run, delay);
  }

  feed(log: MidiLog): void {
    if (this.finished) return;
    const message = log.message;
    const pulse = (source: Source, what: string) => this.done({ source, shape: sourceShape(source.event), what });

    // A parameter (NRPN / RPN) data message beats everything else it is made of.
    const parameter = log.derived.find((d) => d.type === "parameter");
    if (parameter && parameter.type === "parameter") {
      const same =
        this.param &&
        this.param.number === parameter.number &&
        this.param.kind === parameter.kind &&
        this.param.channel === parameter.channel;
      if (!same) {
        this.param = {
          device: log.device,
          channel: parameter.channel,
          kind: parameter.kind,
          number: parameter.number,
          hires: false,
          startedAt: performance.now(),
        };
      }
      if (parameter.change === "data_lsb") this.param!.hires = true;
      this.cc = null;
      const finish = () => this.done(this.paramResult());
      this.emit({
        phase: "detecting",
        preview: this.paramResult(),
        hint: "Keep moving it for a moment…",
      });
      if (performance.now() - this.param!.startedAt > MAX_DETECT_MS) finish();
      else this.settle(finish);
      return;
    }
    if (this.param) return; // Stay with the parameter we are measuring.

    switch (message.type) {
      case "note_on":
        if (message.velocity === 0 || this.cc) return;
        return pulse(
          {
            device: this.device(log.device),
            channel: message.channel, hysteresis: 0,
            event: { type: "note", notes: { min: message.note, max: message.note }, velocity: { min: 1, max: 127 } },
          },
          message.channel === 10 ? "a pad" : "a key",
        );
      case "control_change":
        return this.feedControlChange(log.device, message.channel, message.controller, message.value);
      case "program_change":
        if (this.cc) return;
        return pulse(
          {
            device: this.device(log.device),
            channel: message.channel, hysteresis: 0,
            event: { type: "program_change", programs: { min: message.program, max: message.program } },
          },
          "a program change",
        );
      case "pitch_bend":
        if (this.cc) return;
        return pulse(
          { device: this.device(log.device), channel: message.channel, hysteresis: 0, event: { type: "pitch_bend", range: { ...FULL14 } } },
          "the pitch wheel",
        );
      case "channel_pressure":
        if (this.cc) return;
        return pulse(
          {
            device: this.device(log.device),
            channel: message.channel, hysteresis: 0,
            event: { type: "channel_pressure", range: { min: 1, max: 127 } },
          },
          "aftertouch",
        );
      case "sys_ex":
        return pulse(
          { device: this.device(log.device), channel: null, hysteresis: 0, event: { type: "sys_ex", pattern: bytesToHex(message.data) } },
          "a SysEx message",
        );
      case "start":
      case "stop":
      case "continue":
      case "tune_request":
      case "reset":
        return pulse(
          { device: this.device(log.device), channel: null, hysteresis: 0, event: { type: "system", message: message.type } },
          "a transport button",
        );
      case "song_select":
        return pulse(
          {
            device: this.device(log.device),
            channel: null, hysteresis: 0,
            event: { type: "song_select", songs: { min: message.song, max: message.song } },
          },
          "a song select",
        );
      default:
        // Note offs, poly pressure, clock, active sensing, song position, MTC: not worth learning
        // from on their own (they accompany something else the user did).
        return;
    }
  }

  private paramResult(): LearnResult {
    const p = this.param!;
    const source: Source = {
      device: this.device(p.device),
      channel: p.channel, hysteresis: 0,
      event: { type: p.kind, number: p.number, range: p.hires ? { ...FULL14 } : { ...FULL7 }, hires: p.hires },
    };
    return { source, shape: "continuous", what: `${p.kind.toUpperCase()} parameter ${p.number}` };
  }

  private feedControlChange(device: string, channel: number, controller: number, value: number): void {
    // Parameter-select controllers on their own are almost always the start of an NRPN/RPN
    // message; give the data entry a moment to arrive before treating them as plain CCs.
    const isSelect = controller >= 98 && controller <= 101;
    const current = this.cc;

    if (current && (current.device !== device || current.channel !== channel || current.controller !== controller)) {
      // A 14-bit pair alternates MSB (n) and LSB (n + 32).
      if (
        current.device === device &&
        current.channel === channel &&
        current.controller < 32 &&
        controller === current.controller + 32
      ) {
        current.partnerHits++;
      }
      return; // Stay with the first control touched.
    }

    if (!current) {
      this.cc = { device, channel, controller, values: [value], partnerHits: 0, startedAt: performance.now() };
    } else {
      current.values.push(value);
    }
    const candidate = this.cc!;
    const finish = () => this.done(this.classify(candidate));

    // A momentary button: one "on" value followed by 0.
    if (candidate.values.length === 2 && candidate.values[0] > 0 && value === 0) return finish();

    const preview = this.classify(candidate);
    this.emit({
      phase: "detecting",
      preview,
      hint:
        preview.shape === "button" && candidate.values.length === 1
          ? "Got it — checking what kind of control this is…"
          : "Keep moving it (both directions) for a moment…",
    });
    const elapsed = performance.now() - candidate.startedAt;
    if (candidate.values.length >= ENOUGH_SAMPLES || elapsed > MAX_DETECT_MS) return finish();
    this.settle(finish, isSelect ? SETTLE_MS + 150 : SETTLE_MS);
  }

  private classify(candidate: CcCandidate): LearnResult {
    const { device, channel, controller, values } = candidate;
    const base = { device: this.device(device), channel, hysteresis: 0 };
    const name = ccName(controller);

    if (controller < 32 && candidate.partnerHits >= Math.max(2, values.length - 1)) {
      return {
        source: { ...base, event: { type: "control_change14", controller, range: { ...FULL14 } } },
        shape: "continuous",
        what: "a high-resolution (14-bit) control",
      };
    }

    const encoding = detectRelative(values);
    if (encoding) {
      return {
        source: { ...base, event: { type: "control_change", controller, range: { ...FULL7 }, encoding } },
        shape: "relative",
        what: "an endless encoder",
      };
    }

    const distinct = new Set(values);
    const buttonLike =
      values.length <= 3 &&
      distinct.size <= 2 &&
      [...distinct].every((v) => v === 0 || v === 127 || distinct.size === 1 || distinct.has(0));
    if (buttonLike) {
      const on = Math.max(...values);
      const pedal = controller >= 64 && controller <= 69;
      return {
        source: {
          ...base,
          event: {
            type: "control_change",
            controller,
            range: { min: on >= 64 || on === 0 ? 64 : 1, max: 127 },
            encoding: "absolute",
          },
        },
        shape: "button",
        what: pedal ? `a pedal${name ? ` (${name})` : ""}` : "a button",
      };
    }

    return {
      source: { ...base, event: { type: "control_change", controller, range: { ...FULL7 }, encoding: "absolute" } },
      shape: "continuous",
      what: name ? `a knob or fader (${name})` : "a knob or fader",
    };
  }
}

/**
 * Recognizes the three common relative encodings. Endless encoders repeat a tiny set of values
 * (an absolute control never sends the same value twice in a row), clustered around 0/127 or 64.
 */
function detectRelative(values: number[]): ValueEncoding | null {
  if (values.length < 3) return null;
  let repeats = 0;
  for (let i = 1; i < values.length; i++) if (values[i] === values[i - 1]) repeats++;
  if (repeats < Math.floor((values.length - 1) / 2)) return null;

  const low = values.some((v) => v >= 1 && v <= 15);
  const high = values.some((v) => v >= 113 && v <= 127);
  const aboveCenter = values.some((v) => v >= 65 && v <= 79);
  const belowCenter = values.some((v) => v >= 49 && v <= 63);
  const all = (test: (v: number) => boolean) => values.every(test);

  if (all((v) => (v >= 1 && v <= 15) || (v >= 113 && v <= 127)) && high) return "relative_twos_complement";
  if (all((v) => v >= 49 && v <= 79) && belowCenter) return "relative_binary_offset";
  if (all((v) => (v >= 1 && v <= 15) || (v >= 65 && v <= 79)) && low && aboveCenter) return "relative_sign_magnitude";
  // One direction only: 1s are the same in two's complement and sign-magnitude; 65s are far more
  // often "binary offset +1" than "sign-magnitude -1".
  if (all((v) => v >= 1 && v <= 15)) return "relative_twos_complement";
  if (all((v) => v >= 65 && v <= 79)) return "relative_binary_offset";
  return null;
}

/**
 * Runs a learn session while `active`: tells the engine to hold its fire, feeds incoming MIDI to
 * a `Learner`, and reports progress. `onDone` fires once with the result.
 */
export function useLearn(active: boolean, options: LearnOptions, onDone: (result: LearnResult) => void): LearnState {
  const [state, setState] = useState<LearnState>({ phase: "listening" });
  const callback = useRef(onDone);
  callback.current = onDone;
  const pinDevice = options.pinDevice;

  useEffect(() => {
    if (!active) return;
    setState({ phase: "listening" });
    void api().then((backend) => backend.setLearning(true));
    const learner = new Learner({ pinDevice }, (next) => {
      setState(next);
      if (next.phase === "done") callback.current(next.result);
    });
    const unsubscribe = onMidi((log) => learner.feed(log));
    return () => {
      unsubscribe();
      learner.dispose();
      void api().then((backend) => backend.setLearning(false));
    };
  }, [active, pinDevice]);

  return state;
}
