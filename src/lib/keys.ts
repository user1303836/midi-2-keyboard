// Everything the UI knows about keys: labels (platform aware), grouping for the picker, the
// mapping from browser `KeyboardEvent.code` values, and combo formatting.

import type { Key } from "@/bindings/Key";
import type { MouseButton } from "@/bindings/MouseButton";

import { platform, type Platform } from "./platform";

export type KeyGroup =
  | "Modifiers"
  | "Letters"
  | "Digits"
  | "Navigation"
  | "Editing"
  | "Function"
  | "Punctuation"
  | "Numpad"
  | "Media"
  | "System"
  | "International";

export interface KeyInfo {
  key: Key;
  /** Short label for keycaps. */
  label: string;
  /** Longer name for tooltips and search. */
  name: string;
  group: KeyGroup;
  /** Extra search terms. */
  aliases?: string;
}

type PerPlatform = Partial<Record<Platform, string>>;

interface KeyDef {
  label: string;
  name?: string;
  group: KeyGroup;
  aliases?: string;
  labels?: PerPlatform;
  names?: PerPlatform;
}

const letters = "ABCDEFGHIJKLMNOPQRSTUVWXYZ".split("");

const DEFS: Partial<Record<Key, KeyDef>> = {
  // Modifiers
  ControlLeft: {
    label: "Ctrl",
    name: "Control",
    group: "Modifiers",
    labels: { macos: "⌃" },
    aliases: "ctrl control",
  },
  ShiftLeft: { label: "Shift", group: "Modifiers", labels: { macos: "⇧" }, aliases: "shift" },
  AltLeft: {
    label: "Alt",
    group: "Modifiers",
    labels: { macos: "⌥" },
    names: { macos: "Option" },
    aliases: "alt option opt",
  },
  MetaLeft: {
    label: "Win",
    name: "Windows key",
    group: "Modifiers",
    labels: { macos: "⌘", linux: "Super" },
    names: { macos: "Command", linux: "Super" },
    aliases: "win windows cmd command super meta",
  },
  ControlRight: {
    label: "R Ctrl",
    name: "Right Control",
    group: "Modifiers",
    labels: { macos: "R ⌃" },
    aliases: "ctrl control right",
  },
  ShiftRight: {
    label: "R Shift",
    name: "Right Shift",
    group: "Modifiers",
    labels: { macos: "R ⇧" },
    aliases: "shift right",
  },
  AltRight: {
    label: "R Alt",
    name: "Right Alt (AltGr)",
    group: "Modifiers",
    labels: { macos: "R ⌥" },
    names: { macos: "Right Option" },
    aliases: "alt altgr option right",
  },
  MetaRight: {
    label: "R Win",
    name: "Right Windows key",
    group: "Modifiers",
    labels: { macos: "R ⌘", linux: "R Super" },
    names: { macos: "Right Command", linux: "Right Super" },
    aliases: "win windows cmd command super meta right",
  },

  // Navigation
  ArrowUp: { label: "↑", name: "Up Arrow", group: "Navigation", aliases: "up arrow" },
  ArrowDown: { label: "↓", name: "Down Arrow", group: "Navigation", aliases: "down arrow" },
  ArrowLeft: { label: "←", name: "Left Arrow", group: "Navigation", aliases: "left arrow" },
  ArrowRight: { label: "→", name: "Right Arrow", group: "Navigation", aliases: "right arrow" },
  Home: { label: "Home", group: "Navigation" },
  End: { label: "End", group: "Navigation" },
  PageUp: { label: "PgUp", name: "Page Up", group: "Navigation", aliases: "pageup pgup" },
  PageDown: { label: "PgDn", name: "Page Down", group: "Navigation", aliases: "pagedown pgdn" },

  // Editing
  Enter: {
    label: "Enter",
    group: "Editing",
    labels: { macos: "⏎" },
    names: { macos: "Return" },
    aliases: "enter return",
  },
  Space: { label: "Space", group: "Editing", aliases: "spacebar" },
  Tab: { label: "Tab", group: "Editing" },
  Escape: { label: "Esc", name: "Escape", group: "Editing", aliases: "esc escape" },
  Backspace: {
    label: "Backspace",
    group: "Editing",
    labels: { macos: "⌫" },
    names: { macos: "Delete (Backspace)" },
    aliases: "backspace delete",
  },
  Delete: {
    label: "Del",
    name: "Delete",
    group: "Editing",
    labels: { macos: "⌦" },
    names: { macos: "Forward Delete" },
    aliases: "del delete",
  },
  Insert: { label: "Ins", name: "Insert", group: "Editing", aliases: "ins insert" },
  CapsLock: { label: "Caps", name: "Caps Lock", group: "Editing", aliases: "capslock" },
  ContextMenu: { label: "Menu", name: "Context Menu", group: "Editing", aliases: "menu apps" },
  PrintScreen: { label: "PrtSc", name: "Print Screen", group: "Editing", aliases: "printscreen" },
  ScrollLock: { label: "ScrLk", name: "Scroll Lock", group: "Editing" },
  Pause: { label: "Pause", name: "Pause / Break", group: "Editing", aliases: "break" },

  // Punctuation (US layout positions)
  Backquote: { label: "`", name: "Backquote `", group: "Punctuation", aliases: "grave tilde ~" },
  Minus: { label: "-", name: "Minus -", group: "Punctuation", aliases: "hyphen dash underscore _" },
  Equal: { label: "=", name: "Equals =", group: "Punctuation", aliases: "plus +" },
  BracketLeft: { label: "[", name: "Left Bracket [", group: "Punctuation", aliases: "{" },
  BracketRight: { label: "]", name: "Right Bracket ]", group: "Punctuation", aliases: "}" },
  Backslash: { label: "\\", name: "Backslash \\", group: "Punctuation", aliases: "pipe |" },
  Semicolon: { label: ";", name: "Semicolon ;", group: "Punctuation", aliases: "colon :" },
  Quote: { label: "'", name: "Quote '", group: "Punctuation", aliases: 'apostrophe "' },
  Comma: { label: ",", name: "Comma ,", group: "Punctuation", aliases: "<" },
  Period: { label: ".", name: "Period .", group: "Punctuation", aliases: "dot >" },
  Slash: { label: "/", name: "Slash /", group: "Punctuation", aliases: "question ?" },

  // Numpad
  NumLock: { label: "NumLk", name: "Num Lock", group: "Numpad" },
  NumpadAdd: { label: "Num +", name: "Numpad Add", group: "Numpad", aliases: "plus" },
  NumpadSubtract: { label: "Num −", name: "Numpad Subtract", group: "Numpad", aliases: "minus" },
  NumpadMultiply: { label: "Num ×", name: "Numpad Multiply", group: "Numpad", aliases: "times *" },
  NumpadDivide: { label: "Num ÷", name: "Numpad Divide", group: "Numpad", aliases: "slash /" },
  NumpadDecimal: { label: "Num .", name: "Numpad Decimal", group: "Numpad", aliases: "dot" },
  NumpadEnter: { label: "Num ⏎", name: "Numpad Enter", group: "Numpad" },
  NumpadEqual: { label: "Num =", name: "Numpad Equals", group: "Numpad" },
  NumpadComma: { label: "Num ,", name: "Numpad Comma", group: "Numpad" },

  // Media
  AudioVolumeUp: { label: "Vol +", name: "Volume Up", group: "Media", aliases: "louder" },
  AudioVolumeDown: { label: "Vol −", name: "Volume Down", group: "Media", aliases: "quieter" },
  AudioVolumeMute: { label: "Mute", name: "Mute", group: "Media", aliases: "volume" },
  MediaPlayPause: { label: "⏯", name: "Play / Pause", group: "Media", aliases: "play pause" },
  MediaStop: { label: "⏹", name: "Stop", group: "Media", aliases: "media stop" },
  MediaTrackNext: { label: "⏭", name: "Next Track", group: "Media", aliases: "next skip" },
  MediaTrackPrevious: { label: "⏮", name: "Previous Track", group: "Media", aliases: "prev back" },
  MediaFastForward: { label: "⏩", name: "Fast Forward", group: "Media" },
  MediaRewind: { label: "⏪", name: "Rewind", group: "Media" },
  MediaSelect: { label: "Media", name: "Open Media Player", group: "Media" },
  Eject: { label: "⏏", name: "Eject", group: "Media" },

  // System
  BrightnessUp: { label: "☀+", name: "Brightness Up", group: "System" },
  BrightnessDown: { label: "☀−", name: "Brightness Down", group: "System" },
  BrowserBack: { label: "Back", name: "Browser Back", group: "System" },
  BrowserForward: { label: "Fwd", name: "Browser Forward", group: "System" },
  BrowserRefresh: { label: "Refresh", name: "Browser Refresh", group: "System", aliases: "reload" },
  BrowserStop: { label: "Stop", name: "Browser Stop", group: "System" },
  BrowserSearch: { label: "Search", name: "Browser Search", group: "System" },
  BrowserFavorites: { label: "Favs", name: "Browser Favorites", group: "System" },
  BrowserHome: { label: "Home", name: "Browser Home", group: "System" },
  LaunchMail: { label: "Mail", name: "Launch Mail", group: "System" },
  LaunchApp1: { label: "App 1", name: "Launch App 1 (File Explorer)", group: "System" },
  LaunchApp2: { label: "Calc", name: "Launch App 2 (Calculator)", group: "System" },
  Sleep: { label: "Sleep", group: "System" },
  Power: { label: "Power", group: "System" },
  WakeUp: { label: "Wake", name: "Wake Up", group: "System" },
  Help: { label: "Help", group: "System" },

  // International
  IntlBackslash: {
    label: "ISO \\",
    name: "ISO extra key (next to left Shift)",
    group: "International",
    aliases: "102nd <>",
  },
  IntlRo: { label: "ろ", name: "JIS Ro", group: "International" },
  IntlYen: { label: "¥", name: "JIS Yen", group: "International" },
  Convert: { label: "変換", name: "Convert (Henkan)", group: "International" },
  NonConvert: { label: "無変換", name: "Non-Convert (Muhenkan)", group: "International" },
  KanaMode: { label: "かな", name: "Kana Mode", group: "International" },
  Lang1: { label: "한/영", name: "Lang 1 (Hangul / Kana)", group: "International" },
  Lang2: { label: "한자", name: "Lang 2 (Hanja / Eisu)", group: "International" },
};

/**
 * Legends of the writing-system keys on the user's actual keyboard layout, when the browser can
 * tell us (Chromium's Keyboard Map API). Our keys are physical positions named after the US
 * layout; on AZERTY the key we call `KeyQ` is labelled "A" — and that is what should be shown.
 */
const layoutLegends = new Map<Key, string>();

const LAYOUT_GROUPS: KeyGroup[] = ["Letters", "Digits", "Punctuation", "International"];

/** Loads the layout legends. Resolves quickly either way; safe to call more than once. */
export async function loadKeyboardLayout(): Promise<void> {
  type LayoutMap = { entries(): IterableIterator<[string, string]> };
  const keyboard = (navigator as unknown as { keyboard?: { getLayoutMap?: () => Promise<LayoutMap> } }).keyboard;
  if (!keyboard?.getLayoutMap) return;
  try {
    const layout = await Promise.race([
      keyboard.getLayoutMap(),
      new Promise<null>((resolve) => setTimeout(() => resolve(null), 800)),
    ]);
    if (!layout) return;
    layoutLegends.clear();
    for (const [code, legend] of layout.entries()) {
      // Dead keys and multi-character legends are not worth showing on a keycap.
      if ([...legend].length === 1 && legend.trim() !== "") {
        layoutLegends.set(code as Key, legend.toLocaleUpperCase());
      }
    }
    cache = null;
  } catch {
    // Not available here: the US names stay.
  }
}

function build(): Map<Key, KeyInfo> {
  const os = platform();
  const map = new Map<Key, KeyInfo>();
  const add = (key: Key, def: KeyDef) => {
    let label = def.labels?.[os] ?? def.label;
    let name = def.names?.[os] ?? def.name ?? def.label;
    let aliases = def.aliases;
    const legend = LAYOUT_GROUPS.includes(def.group) ? layoutLegends.get(key) : undefined;
    if (legend && legend !== def.label.toLocaleUpperCase()) {
      // Keep the US name searchable and visible in the tooltip.
      aliases = `${aliases ?? ""} ${def.label} ${name}`;
      name = `${legend}  (the ${def.label} key on a US keyboard)`;
      label = legend;
    }
    map.set(key, { key, label, name, group: def.group, aliases });
  };
  // Ordered the way the picker should list them.
  (
    [
      "ControlLeft",
      "ShiftLeft",
      "AltLeft",
      "MetaLeft",
      "ControlRight",
      "ShiftRight",
      "AltRight",
      "MetaRight",
    ] as Key[]
  ).forEach((key) => add(key, DEFS[key]!));
  for (const letter of letters) {
    add(`Key${letter}` as Key, { label: letter, group: "Letters" });
  }
  for (let digit = 0; digit <= 9; digit++) {
    add(`Digit${digit}` as Key, { label: String(digit), group: "Digits" });
  }
  for (let n = 1; n <= 24; n++) {
    add(`F${n}` as Key, { label: `F${n}`, group: "Function" });
  }
  for (let digit = 0; digit <= 9; digit++) {
    add(`Numpad${digit}` as Key, {
      label: `Num ${digit}`,
      name: `Numpad ${digit}`,
      group: "Numpad",
    });
  }
  for (const [key, def] of Object.entries(DEFS) as [Key, KeyDef][]) {
    if (!map.has(key)) add(key, def);
  }
  return map;
}

let cache: Map<Key, KeyInfo> | null = null;
let cachePlatform: Platform | null = null;

function table(): Map<Key, KeyInfo> {
  if (!cache || cachePlatform !== platform()) {
    cache = build();
    cachePlatform = platform();
  }
  return cache;
}

export function keyInfo(key: Key): KeyInfo {
  return table().get(key) ?? { key, label: key, name: key, group: "System" };
}

export function keyLabel(key: Key): string {
  return keyInfo(key).label;
}

export function allKeys(): KeyInfo[] {
  return Array.from(table().values());
}

export const KEY_GROUPS: KeyGroup[] = [
  "Modifiers",
  "Letters",
  "Digits",
  "Navigation",
  "Editing",
  "Function",
  "Punctuation",
  "Numpad",
  "Media",
  "System",
  "International",
];

const MODIFIER_ORDER: Key[] = [
  "ControlLeft",
  "ControlRight",
  "AltLeft",
  "AltRight",
  "ShiftLeft",
  "ShiftRight",
  "MetaLeft",
  "MetaRight",
];

export function isModifier(key: Key): boolean {
  return MODIFIER_ORDER.includes(key);
}

/** Modifiers first (in the conventional order), then the other keys in the order given. */
export function normalizeCombo(keys: readonly Key[]): Key[] {
  const unique = Array.from(new Set(keys));
  const modifiers = MODIFIER_ORDER.filter((key) => unique.includes(key));
  const rest = unique.filter((key) => !isModifier(key));
  return [...modifiers, ...rest];
}

/** Plain-text combo, e.g. "Ctrl+Shift+O". */
export function comboText(keys: readonly Key[]): string {
  if (keys.length === 0) return "nothing";
  const separator = platform() === "macos" ? " " : "+";
  return keys.map(keyLabel).join(separator);
}

// Browser `code` values that differ from ours (legacy / engine specific spellings).
const CODE_ALIASES: Record<string, Key> = {
  OSLeft: "MetaLeft",
  OSRight: "MetaRight",
  VolumeUp: "AudioVolumeUp",
  VolumeDown: "AudioVolumeDown",
  VolumeMute: "AudioVolumeMute",
  NumpadClear: "NumLock",
  Props: "ContextMenu",
};

/** Maps a browser keyboard event to one of our keys, or `null` if we cannot send that key. */
export function keyFromEvent(event: KeyboardEvent): Key | null {
  const code = CODE_ALIASES[event.code] ?? event.code;
  return table().has(code as Key) ? (code as Key) : null;
}

export const MOUSE_BUTTONS: { value: MouseButton; label: string }[] = [
  { value: "left", label: "Left" },
  { value: "right", label: "Right" },
  { value: "middle", label: "Middle" },
  { value: "back", label: "Back" },
  { value: "forward", label: "Forward" },
];

export function mouseButtonLabel(button: MouseButton): string {
  return MOUSE_BUTTONS.find((b) => b.value === button)?.label ?? button;
}
