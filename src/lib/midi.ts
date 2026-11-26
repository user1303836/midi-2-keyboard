// MIDI presentation helpers: note / controller names, message formatting, encoding to bytes.

import type { MidiMessage } from "@/bindings/MidiMessage";

import { hex } from "./utils";

const NOTE_NAMES = ["C", "C♯", "D", "D♯", "E", "F", "F♯", "G", "G♯", "A", "A♯", "B"];

/** `C4` for note 60 when `middleCOctave` is 4 (scientific) — 3 gives the Yamaha convention. */
export function noteName(note: number, middleCOctave = 4): string {
  const octave = Math.floor(note / 12) - 5 + middleCOctave;
  return `${NOTE_NAMES[note % 12]}${octave}`;
}

export function isBlackKey(note: number): boolean {
  return [1, 3, 6, 8, 10].includes(note % 12);
}

const CC_NAMES: Record<number, string> = {
  0: "Bank Select",
  1: "Mod Wheel",
  2: "Breath",
  4: "Foot Controller",
  5: "Portamento Time",
  6: "Data Entry",
  7: "Volume",
  8: "Balance",
  10: "Pan",
  11: "Expression",
  12: "Effect 1",
  13: "Effect 2",
  32: "Bank Select LSB",
  38: "Data Entry LSB",
  64: "Sustain Pedal",
  65: "Portamento",
  66: "Sostenuto",
  67: "Soft Pedal",
  68: "Legato",
  69: "Hold 2",
  70: "Sound Variation",
  71: "Resonance",
  72: "Release",
  73: "Attack",
  74: "Cutoff",
  84: "Portamento Control",
  91: "Reverb",
  92: "Tremolo",
  93: "Chorus",
  94: "Detune",
  95: "Phaser",
  96: "Data Increment",
  97: "Data Decrement",
  98: "NRPN LSB",
  99: "NRPN MSB",
  100: "RPN LSB",
  101: "RPN MSB",
  120: "All Sound Off",
  121: "Reset Controllers",
  122: "Local Control",
  123: "All Notes Off",
  124: "Omni Off",
  125: "Omni On",
  126: "Mono On",
  127: "Poly On",
};

/** Conventional name of a controller, or `""` when it has none. */
export function ccName(controller: number): string {
  return CC_NAMES[controller] ?? "";
}

const RPN_NAMES: Record<number, string> = {
  0: "Pitch Bend Range",
  1: "Fine Tuning",
  2: "Coarse Tuning",
  3: "Tuning Program",
  4: "Tuning Bank",
  5: "Mod Depth Range",
  6: "MPE Configuration",
};

export function rpnName(parameter: number): string {
  return RPN_NAMES[parameter] ?? "";
}

export type MessageKind =
  | "note"
  | "cc"
  | "program"
  | "bend"
  | "pressure"
  | "sysex"
  | "clock"
  | "transport"
  | "system";

export interface MessageView {
  kind: MessageKind;
  /** Short type label, e.g. "Note On". */
  label: string;
  /** The main datum, e.g. "C4" or "CC 20". */
  subject: string;
  /** Secondary data, e.g. "vel 100" or "= 64". */
  detail: string;
  channel: number | null;
}

/** Breaks a message into display parts. */
export function viewMessage(message: MidiMessage, middleCOctave = 4): MessageView {
  switch (message.type) {
    case "note_on":
      return {
        kind: "note",
        label: message.velocity === 0 ? "Note Off" : "Note On",
        subject: noteName(message.note, middleCOctave),
        detail: message.velocity === 0 ? "vel 0" : `vel ${message.velocity}`,
        channel: message.channel,
      };
    case "note_off":
      return {
        kind: "note",
        label: "Note Off",
        subject: noteName(message.note, middleCOctave),
        detail: message.velocity ? `vel ${message.velocity}` : "",
        channel: message.channel,
      };
    case "poly_pressure":
      return {
        kind: "pressure",
        label: "Poly Pressure",
        subject: noteName(message.note, middleCOctave),
        detail: `= ${message.pressure}`,
        channel: message.channel,
      };
    case "control_change": {
      const name = ccName(message.controller);
      return {
        kind: "cc",
        label: "CC",
        subject: `CC ${message.controller}${name ? ` · ${name}` : ""}`,
        detail: `= ${message.value}`,
        channel: message.channel,
      };
    }
    case "program_change":
      return {
        kind: "program",
        label: "Program",
        subject: `Program ${message.program}`,
        detail: "",
        channel: message.channel,
      };
    case "channel_pressure":
      return {
        kind: "pressure",
        label: "Pressure",
        subject: "Channel Pressure",
        detail: `= ${message.pressure}`,
        channel: message.channel,
      };
    case "pitch_bend":
      return {
        kind: "bend",
        label: "Pitch Bend",
        subject: "Pitch Bend",
        detail: `= ${message.value - 8192 > 0 ? "+" : ""}${message.value - 8192}`,
        channel: message.channel,
      };
    case "sys_ex":
      return {
        kind: "sysex",
        label: "SysEx",
        subject: `${message.data.length} bytes`,
        detail: bytesToHex(message.data.slice(0, 12)) + (message.data.length > 12 ? " …" : ""),
        channel: null,
      };
    case "mtc_quarter_frame":
      return {
        kind: "system",
        label: "MTC",
        subject: "Quarter Frame",
        detail: `${message.piece}:${message.value}`,
        channel: null,
      };
    case "song_position":
      return {
        kind: "system",
        label: "Song Position",
        subject: "Song Position",
        detail: `= ${message.position}`,
        channel: null,
      };
    case "song_select":
      return {
        kind: "system",
        label: "Song Select",
        subject: `Song ${message.song}`,
        detail: "",
        channel: null,
      };
    case "tune_request":
      return { kind: "system", label: "Tune Request", subject: "Tune Request", detail: "", channel: null };
    case "clock":
      return { kind: "clock", label: "Clock", subject: "Clock", detail: "", channel: null };
    case "start":
      return { kind: "transport", label: "Start", subject: "Start", detail: "", channel: null };
    case "continue":
      return { kind: "transport", label: "Continue", subject: "Continue", detail: "", channel: null };
    case "stop":
      return { kind: "transport", label: "Stop", subject: "Stop", detail: "", channel: null };
    case "active_sensing":
      return { kind: "clock", label: "Active Sensing", subject: "Active Sensing", detail: "", channel: null };
    case "reset":
      return { kind: "system", label: "Reset", subject: "System Reset", detail: "", channel: null };
    case "unknown":
      return {
        kind: "system",
        label: "Unknown",
        subject: "Unknown",
        detail: bytesToHex(message.data.slice(0, 12)),
        channel: null,
      };
  }
}

/** Wire bytes of a message (channels are 1..=16 in our types). */
export function encodeMessage(message: MidiMessage): number[] {
  const ch = (channel: number) => (channel - 1) & 0x0f;
  switch (message.type) {
    case "note_off":
      return [0x80 | ch(message.channel), message.note, message.velocity];
    case "note_on":
      return [0x90 | ch(message.channel), message.note, message.velocity];
    case "poly_pressure":
      return [0xa0 | ch(message.channel), message.note, message.pressure];
    case "control_change":
      return [0xb0 | ch(message.channel), message.controller, message.value];
    case "program_change":
      return [0xc0 | ch(message.channel), message.program];
    case "channel_pressure":
      return [0xd0 | ch(message.channel), message.pressure];
    case "pitch_bend":
      return [0xe0 | ch(message.channel), message.value & 0x7f, (message.value >> 7) & 0x7f];
    case "sys_ex":
      return message.data;
    case "mtc_quarter_frame":
      return [0xf1, ((message.piece & 0x07) << 4) | (message.value & 0x0f)];
    case "song_position":
      return [0xf2, message.position & 0x7f, (message.position >> 7) & 0x7f];
    case "song_select":
      return [0xf3, message.song];
    case "tune_request":
      return [0xf6];
    case "clock":
      return [0xf8];
    case "start":
      return [0xfa];
    case "continue":
      return [0xfb];
    case "stop":
      return [0xfc];
    case "active_sensing":
      return [0xfe];
    case "reset":
      return [0xff];
    case "unknown":
      return message.data;
  }
}

export function bytesToHex(bytes: readonly number[]): string {
  return bytes.map(hex).join(" ");
}

/** Validates and normalizes a SysEx pattern (`F0 7F ?? 06 02 F7`, `??` = any byte, `**` = any run). */
export function normalizeSysExPattern(input: string): { pattern: string; error: string | null } {
  const tokens = input
    .toUpperCase()
    .replace(/,/g, " ")
    .split(/\s+/)
    .filter(Boolean);
  if (tokens.length === 0) return { pattern: "", error: "Enter at least one byte" };
  for (const token of tokens) {
    if (token === "??" || token === "**") continue;
    if (!/^[0-9A-F]{1,2}$/.test(token)) {
      return { pattern: tokens.join(" "), error: `"${token}" is not a hex byte, ?? or **` };
    }
  }
  const normalized = tokens.map((t) => (t.length === 1 && t !== "?" ? `0${t}` : t));
  return { pattern: normalized.join(" "), error: null };
}

/** Well-known SysEx messages worth offering as presets (MIDI Machine Control). */
export const SYSEX_PRESETS: { label: string; pattern: string }[] = [
  { label: "MMC Stop", pattern: "F0 7F ?? 06 01 F7" },
  { label: "MMC Play", pattern: "F0 7F ?? 06 02 F7" },
  { label: "MMC Deferred Play", pattern: "F0 7F ?? 06 03 F7" },
  { label: "MMC Fast Forward", pattern: "F0 7F ?? 06 04 F7" },
  { label: "MMC Rewind", pattern: "F0 7F ?? 06 05 F7" },
  { label: "MMC Record", pattern: "F0 7F ?? 06 06 F7" },
  { label: "MMC Record Stop", pattern: "F0 7F ?? 06 07 F7" },
  { label: "MMC Pause", pattern: "F0 7F ?? 06 09 F7" },
];
