import {
  ArrowDown,
  ArrowLeftRight,
  ArrowUp,
  Cable,
  ChevronDown,
  ExternalLink,
  Keyboard,
  Layers,
  ListOrdered,
  Mouse,
  MousePointerClick,
  Move,
  Pause,
  Plus,
  Terminal,
  Trash2,
  Type,
} from "lucide-react";
import { createContext, useContext, useState, type ReactNode } from "react";

import type { Action } from "@/bindings/Action";
import type { EngineOp } from "@/bindings/EngineOp";
import type { HoldTarget } from "@/bindings/HoldTarget";
import type { LayerOp } from "@/bindings/LayerOp";
import type { MouseButton } from "@/bindings/MouseButton";
import type { SequenceStep } from "@/bindings/SequenceStep";
import type { Source } from "@/bindings/Source";
import { Button, IconButton } from "@/components/ui/button";
import { NumberInput, Segmented, Select, Switch, TextArea, TextInput } from "@/components/ui/inputs";
import { Menu, MenuContent, MenuItem, MenuLabel, MenuTrigger, Tooltip } from "@/components/ui/overlays";
import { MOUSE_BUTTONS } from "@/lib/keys";
import {
  ACTION_TYPES,
  actionTypeInfo,
  defaultAction,
  defaultStep,
  midiSendForSource,
  STEP_TYPES,
  type ActionType,
  type StepType,
} from "@/lib/model";
import { useApp, useKnownLayers } from "@/lib/store";
import { cn, move } from "@/lib/utils";

import { KeyComboInput } from "./KeyComboInput";
import { MidiSendEditor, OutputField } from "./MidiSendEditor";

/**
 * The source of the mapping whose actions are being edited, where there is one: a fresh
 * "Send MIDI" starts out as that very control, which is what it is nearly always for.
 */
export const SourceContext = createContext<Source | null>(null);

const ACTION_ICONS: Record<ActionType, ReactNode> = {
  keys: <Keyboard size={15} />,
  text: <Type size={15} />,
  sequence: <ListOrdered size={15} />,
  mouse_click: <MousePointerClick size={15} />,
  mouse_scroll: <Mouse size={15} />,
  mouse_move: <Move size={15} />,
  switch_profile: <ArrowLeftRight size={15} />,
  layer: <Layers size={15} />,
  engine: <Pause size={15} />,
  run: <Terminal size={15} />,
  open: <ExternalLink size={15} />,
  midi: <Cable size={15} />,
};

const TARGET_ICONS: Record<HoldTarget["type"], ReactNode> = {
  keys: <Keyboard size={15} />,
  mouse_button: <MousePointerClick size={15} />,
  layer: <Layers size={15} />,
};

/** What the engine fills in while typing text, with what each one stands for. */
const TEXT_PLACEHOLDERS: [string, string][] = [
  ["value", "The value of the message: a knob's position, a note's velocity, a program number"],
  ["note", "The note number, e.g. 60"],
  ["note_name", "The note name, e.g. C4"],
  ["velocity", "How hard the note was hit, 1–127"],
  ["channel", "The MIDI channel, 1–16"],
  ["controller", "The controller number of a control change"],
];

function unknownPlaceholders(text: string): string[] {
  const known = new Set(TEXT_PLACEHOLDERS.map(([name]) => name));
  const found = new Set<string>();
  for (const match of text.matchAll(/\{([a-z_]+)\}/gi)) {
    if (!known.has(match[1])) found.add(match[1]);
  }
  return [...found].slice(0, 3);
}

/** The compact "what kind of thing is this" button at the end of an action row. */
function TypeMenu({
  label,
  icon,
  tall,
  children,
}: {
  label: string;
  icon: ReactNode;
  tall: boolean;
  children: ReactNode;
}) {
  return (
    <Menu>
      <Tooltip content={label}>
        <MenuTrigger
          aria-label={label}
          className={cn(
            "inline-flex w-[44px] shrink-0 items-center justify-center gap-0.5 rounded-md border border-line-strong bg-app text-fg-muted transition-colors",
            "hover:border-fg-faint hover:text-fg data-[state=open]:border-accent data-[state=open]:text-fg",
            tall ? "h-[34px]" : "h-[28px]",
          )}
        >
          {icon}
          <ChevronDown size={11} className="text-fg-faint" />
        </MenuTrigger>
      </Tooltip>
      <MenuContent className="w-[270px]">{children}</MenuContent>
    </Menu>
  );
}

const ENGINE_OPTIONS: { value: EngineOp; label: string; hint: string }[] = [
  { value: "toggle_pause", label: "Pause / resume", hint: "Flip between sending and not sending" },
  { value: "pause", label: "Pause", hint: "Stop sending output" },
  { value: "resume", label: "Resume", hint: "Start sending output again" },
  { value: "release_all", label: "Release all keys", hint: "Panic button: let go of everything held" },
];

const BUTTON_OPTIONS = MOUSE_BUTTONS.map((b) => ({ value: b.value, label: b.label }));

/** Small inline label used inside action forms. */
function Label({ children }: { children: React.ReactNode }) {
  return <span className="shrink-0 text-sm text-fg-muted">{children}</span>;
}

export function LayerInput({ value, onChange }: { value: string; onChange: (value: string) => void }) {
  const layers = useKnownLayers();
  const listId = "m2k-layer-names";
  return (
    <>
      <TextInput
        aria-label="Layer name"
        className="w-[150px]"
        list={listId}
        placeholder="layer name"
        value={value}
        onChange={onChange}
      />
      <datalist id={listId}>
        {layers.map((layer) => (
          <option key={layer} value={layer} />
        ))}
      </datalist>
    </>
  );
}

type ScrollDirection = "up" | "down" | "left" | "right";

function ScrollFields({
  dx,
  dy,
  onChange,
}: {
  dx: number;
  dy: number;
  onChange: (dx: number, dy: number) => void;
}) {
  const direction: ScrollDirection = dy > 0 ? "up" : dy < 0 ? "down" : dx > 0 ? "right" : dx < 0 ? "left" : "up";
  const amount = Math.max(1, Math.abs(dy) || Math.abs(dx));
  const apply = (dir: ScrollDirection, n: number) => {
    if (dir === "up") onChange(0, n);
    else if (dir === "down") onChange(0, -n);
    else if (dir === "right") onChange(n, 0);
    else onChange(-n, 0);
  };
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Segmented
        value={direction}
        options={[
          { value: "up", label: "Up" },
          { value: "down", label: "Down" },
          { value: "left", label: "Left" },
          { value: "right", label: "Right" },
        ]}
        onChange={(dir) => apply(dir, amount)}
      />
      <NumberInput
        aria-label="Scroll amount"
        className="w-[92px]"
        suffix="notch"
        min={1}
        max={100}
        value={amount}
        onChange={(n) => apply(direction, n)}
      />
    </div>
  );
}

function MoveFields({
  x,
  y,
  absolute,
  onChange,
}: {
  x: number;
  y: number;
  absolute: boolean;
  onChange: (x: number, y: number, absolute: boolean) => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Segmented
        value={absolute ? "to" : "by"}
        options={[
          { value: "by", label: "Move by", title: "Relative to where the pointer is" },
          { value: "to", label: "Move to", title: "An absolute screen position" },
        ]}
        onChange={(mode) => onChange(x, y, mode === "to")}
      />
      <Label>x</Label>
      <NumberInput aria-label="X" className="w-[72px]" min={-20000} max={20000} value={x} onChange={(v) => onChange(v, y, absolute)} />
      <Label>y</Label>
      <NumberInput aria-label="Y" className="w-[72px]" min={-20000} max={20000} value={y} onChange={(v) => onChange(x, v, absolute)} />
      <Label>px</Label>
    </div>
  );
}

function ClickFields({
  button,
  clicks,
  onChange,
}: {
  button: MouseButton;
  clicks: number;
  onChange: (button: MouseButton, clicks: number) => void;
}) {
  return (
    <div className="flex flex-wrap items-center gap-2">
      <Select
        aria-label="Mouse button"
        className="w-[110px]"
        value={button}
        options={BUTTON_OPTIONS}
        onChange={(next) => onChange(next, clicks)}
      />
      <Segmented
        value={String(Math.min(3, Math.max(1, clicks)))}
        options={[
          { value: "1", label: "Single" },
          { value: "2", label: "Double" },
          { value: "3", label: "Triple" },
        ]}
        onChange={(value) => onChange(button, Number(value))}
      />
    </div>
  );
}

/** "Held for N ms" override shown under a key combo. */
function TapTime({ value, onChange }: { value: number | null; onChange: (value: number | null) => void }) {
  const fallback = useApp((state) => state.config.settings.tap_ms);
  if (value === null) {
    return (
      <div className="text-sm text-fg-faint">
        Key held for {fallback} ms ·{" "}
        <button type="button" className="underline underline-offset-2 hover:text-fg" onClick={() => onChange(fallback)}>
          change
        </button>
      </div>
    );
  }
  return (
    <div className="flex items-center gap-2 text-sm text-fg-muted">
      Key held for
      <NumberInput aria-label="Hold time" suffix="ms" className="w-[84px]" min={1} max={5000} value={value} onChange={onChange} />
      <button type="button" className="text-fg-faint underline-offset-2 hover:text-fg hover:underline" onClick={() => onChange(null)}>
        use default
      </button>
    </div>
  );
}

export function ActionEditor({
  action,
  onChange,
  autoCapture = false,
  onCaptured,
}: {
  action: Action;
  onChange: (action: Action, coalesceKey?: string) => void;
  autoCapture?: boolean;
  onCaptured?: () => void;
}) {
  const profiles = useApp((state) => state.config.profiles);
  const viewProfileId = useApp((state) => state.viewProfileId);
  const source = useContext(SourceContext);

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-start gap-1.5">
        <div className="min-w-0 flex-1">
          {action.type === "keys" && (
            <KeyComboInput
              keys={action.keys}
              autoCapture={autoCapture}
              onCaptured={onCaptured}
              onChange={(keys) => onChange({ ...action, keys })}
            />
          )}
          {action.type === "text" && (
            <TextArea
              aria-label="Text to type"
              placeholder="Text to type…"
              value={action.text}
              onChange={(text) => onChange({ ...action, text }, "text")}
            />
          )}
          {action.type === "mouse_click" && (
            <ClickFields
              button={action.button}
              clicks={action.clicks}
              onChange={(button, clicks) => onChange({ ...action, button, clicks })}
            />
          )}
          {action.type === "mouse_scroll" && (
            <ScrollFields dx={action.dx} dy={action.dy} onChange={(dx, dy) => onChange({ ...action, dx, dy }, "scroll")} />
          )}
          {action.type === "mouse_move" && (
            <MoveFields
              x={action.x}
              y={action.y}
              absolute={action.absolute}
              onChange={(x, y, absolute) => onChange({ ...action, x, y, absolute }, "move")}
            />
          )}
          {action.type === "switch_profile" && (
            <Select
              aria-label="Profile to switch to"
              value={action.target.type === "profile" ? `id:${action.target.id}` : action.target.type}
              options={[
                { value: "next", label: "Next profile", group: "Relative" },
                { value: "previous", label: "Previous profile", group: "Relative" },
                {
                  value: "by_value",
                  label: "Picked by the value",
                  hint: "Value 0 = first profile, 1 = second… (program N selects profile N)",
                  group: "Relative",
                },
                ...profiles
                  .filter((p) => !p.always_on)
                  .map((p) => ({ value: `id:${p.id}`, label: p.name, group: "Profile" })),
              ]}
              onChange={(value) =>
                onChange({
                  ...action,
                  target:
                    value === "next" || value === "previous" || value === "by_value"
                      ? { type: value }
                      : { type: "profile", id: value.slice(3) },
                })
              }
            />
          )}
          {action.type === "layer" && (
            <div className="flex flex-wrap items-center gap-2">
              <Segmented<LayerOp>
                value={action.op}
                options={[
                  { value: "toggle", label: "Toggle" },
                  { value: "on", label: "Turn on" },
                  { value: "off", label: "Turn off" },
                ]}
                onChange={(op) => onChange({ ...action, op })}
              />
              <LayerInput value={action.layer} onChange={(layer) => onChange({ ...action, layer }, "layer")} />
            </div>
          )}
          {action.type === "engine" && (
            <Select aria-label="Engine action" value={action.op} options={ENGINE_OPTIONS} onChange={(op) => onChange({ ...action, op })} />
          )}
          {action.type === "run" && (
            <div className="flex flex-col gap-1.5">
              <TextInput
                aria-label="Program"
                mono
                placeholder="Program, e.g. notepad.exe or /usr/bin/firefox"
                value={action.command}
                onChange={(command) => onChange({ ...action, command }, "command")}
              />
              <TextArea
                aria-label="Arguments"
                mono
                rows={2}
                placeholder="Arguments, one per line (optional)"
                value={action.args.join("\n")}
                onChange={(text) => onChange({ ...action, args: text.split("\n") }, "args")}
                onBlur={() => onChange({ ...action, args: action.args.filter((arg) => arg.length > 0) })}
              />
              <TextInput
                aria-label="Working directory"
                mono
                placeholder="Working directory (optional)"
                value={action.working_dir}
                onChange={(working_dir) => onChange({ ...action, working_dir }, "cwd")}
              />
            </div>
          )}
          {action.type === "open" && (
            <TextInput
              aria-label="File, folder or URL"
              mono
              placeholder="https://… or a file / folder path"
              value={action.target}
              onChange={(target) => onChange({ ...action, target }, "target")}
            />
          )}
          {action.type === "sequence" && (
            <SequenceEditor steps={action.steps} onChange={(steps, key) => onChange({ ...action, steps }, key)} />
          )}
          {action.type === "midi" && (
            <div className="flex flex-col gap-1.5">
              <MidiSendEditor
                message={action.message}
                onChange={(message, key) => onChange({ ...action, message }, key && `midi:${key}`)}
              />
              <OutputField value={action.device} onChange={(device) => onChange({ ...action, device })} />
            </div>
          )}
        </div>
        <TypeMenu
          label={`Action: ${actionTypeInfo(action.type).label}. Click to send something else`}
          tall={action.type === "keys"}
          icon={ACTION_ICONS[action.type]}
        >
          {(["Keyboard", "Mouse", "App", "System", "MIDI"] as const).map((group) => (
            <div key={group}>
              <MenuLabel>{group}</MenuLabel>
              {ACTION_TYPES.filter((info) => info.group === group).map((info) => (
                <MenuItem
                  key={info.type}
                  icon={ACTION_ICONS[info.type]}
                  className="h-auto py-1"
                  onSelect={() => {
                    if (info.type === action.type) return;
                    onChange(
                      info.type === "midi" && source
                        ? { type: "midi", device: null, message: midiSendForSource(source) }
                        : defaultAction(info.type, viewProfileId ?? ""),
                    );
                  }}
                >
                  <div>
                    <div className={cn(info.type === action.type && "font-semibold text-accent")}>{info.label}</div>
                    <div className="text-sm text-fg-faint">{info.hint}</div>
                  </div>
                </MenuItem>
              ))}
            </div>
          ))}
        </TypeMenu>
      </div>

      {action.type === "keys" && action.keys.length > 0 && (
        <TapTime value={action.tap_ms} onChange={(tap_ms) => onChange({ ...action, tap_ms }, "tap")} />
      )}
      {action.type === "text" && (
        <div className="flex flex-wrap items-center gap-2 text-sm text-fg-faint">
          <span>Pause between characters</span>
          <NumberInput
            aria-label="Delay between characters"
            suffix="ms"
            className="w-[84px]"
            min={0}
            max={2000}
            value={action.char_delay_ms}
            onChange={(char_delay_ms) => onChange({ ...action, char_delay_ms }, "chardelay")}
          />
        </div>
      )}
      {action.type === "text" && (
        <div className="flex flex-col gap-1 text-sm text-fg-faint">
          <div className="flex flex-wrap items-center gap-1">
            <span className="mr-0.5">Insert from the MIDI message:</span>
            {TEXT_PLACEHOLDERS.map(([name, meaning]) => (
              <button
                key={name}
                type="button"
                title={meaning}
                className="rounded border border-line-strong bg-app px-1 font-mono text-xs text-fg-muted hover:border-accent hover:text-fg"
                onClick={() => onChange({ ...action, text: `${action.text}{${name}}` })}
              >
                {`{${name}}`}
              </button>
            ))}
          </div>
          {unknownPlaceholders(action.text).map((name) => (
            <div key={name} className="text-warn">
              {`{${name}}`} is not a placeholder — it is typed exactly as written.
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** Editor for something that is held: keys, a mouse button or a layer. */
export function TargetEditor({
  target,
  onChange,
  autoCapture = false,
  onCaptured,
}: {
  target: HoldTarget;
  onChange: (target: HoldTarget, coalesceKey?: string) => void;
  autoCapture?: boolean;
  onCaptured?: () => void;
}) {
  return (
    <div className="flex items-start gap-1.5">
      <div className="min-w-0 flex-1">
        {target.type === "keys" && (
          <KeyComboInput
            keys={target.keys}
            autoCapture={autoCapture}
            onCaptured={onCaptured}
            placeholder="Click, then press the key(s) to hold"
            onChange={(keys) => onChange({ ...target, keys })}
          />
        )}
        {target.type === "mouse_button" && (
          <Select
            aria-label="Mouse button"
            value={target.button}
            options={BUTTON_OPTIONS}
            onChange={(button) => onChange({ ...target, button })}
          />
        )}
        {target.type === "layer" && (
          <LayerInput value={target.layer} onChange={(layer) => onChange({ ...target, layer }, "layer")} />
        )}
      </div>
      <TypeMenu
        label="What to hold. Click to hold something else"
        tall={target.type === "keys"}
        icon={TARGET_ICONS[target.type]}
      >
        {(
          [
            ["keys", "Keys", "Hold keys down"],
            ["mouse_button", "Mouse button", "Hold a mouse button (for dragging)"],
            ["layer", "Layer", "A shift layer other mappings can depend on"],
          ] as const
        ).map(([type, label, hint]) => (
          <MenuItem
            key={type}
            icon={TARGET_ICONS[type]}
            className="h-auto py-1"
            onSelect={() =>
              type !== target.type &&
              onChange(
                type === "keys"
                  ? { type, keys: [] }
                  : type === "mouse_button"
                    ? { type, button: "left" }
                    : { type, layer: "shift" },
              )
            }
          >
            <div>
              <div className={cn(type === target.type && "font-semibold text-accent")}>{label}</div>
              <div className="text-sm text-fg-faint">{hint}</div>
            </div>
          </MenuItem>
        ))}
      </TypeMenu>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Macro
// ---------------------------------------------------------------------------------------------

function StepEditor({ step, onChange }: { step: SequenceStep; onChange: (step: SequenceStep, key?: string) => void }) {
  switch (step.type) {
    case "keys":
      return <KeyComboInput keys={step.keys} onChange={(keys) => onChange({ ...step, keys })} />;
    case "key_down":
    case "key_up":
      return <KeyComboInput single keys={[step.key]} onChange={(keys) => keys[0] && onChange({ ...step, key: keys[0] })} />;
    case "text":
      return <TextInput aria-label="Text" placeholder="Text to type…" value={step.text} onChange={(text) => onChange({ ...step, text }, "text")} />;
    case "delay":
      return (
        <NumberInput aria-label="Delay" suffix="ms" className="w-[100px]" min={1} max={600000} value={step.ms} onChange={(ms) => onChange({ ...step, ms }, "ms")} />
      );
    case "mouse_click":
      return <ClickFields button={step.button} clicks={step.clicks} onChange={(button, clicks) => onChange({ ...step, button, clicks })} />;
    case "mouse_down":
    case "mouse_up":
      return (
        <Select aria-label="Mouse button" className="w-[110px]" value={step.button} options={BUTTON_OPTIONS} onChange={(button) => onChange({ ...step, button })} />
      );
    case "mouse_scroll":
      return <ScrollFields dx={step.dx} dy={step.dy} onChange={(dx, dy) => onChange({ ...step, dx, dy }, "scroll")} />;
    case "mouse_move":
      return (
        <MoveFields x={step.x} y={step.y} absolute={step.absolute} onChange={(x, y, absolute) => onChange({ ...step, x, y, absolute }, "move")} />
      );
    case "midi":
      return (
        <div className="flex flex-col gap-1.5">
          <MidiSendEditor message={step.message} onChange={(message, key) => onChange({ ...step, message }, key && `midi:${key}`)} />
          <OutputField value={step.device} onChange={(device) => onChange({ ...step, device })} />
        </div>
      );
  }
}

function SequenceEditor({
  steps,
  onChange,
}: {
  steps: SequenceStep[];
  onChange: (steps: SequenceStep[], coalesceKey?: string) => void;
}) {
  const [hovered, setHovered] = useState<number | null>(null);
  const setStep = (index: number, step: SequenceStep, key?: string) =>
    onChange(steps.map((s, i) => (i === index ? step : s)), key ? `step${index}:${key}` : undefined);
  const stepLabel = (type: StepType) => STEP_TYPES.find((t) => t.type === type)!.label;

  return (
    <div className="flex flex-col gap-1.5">
      {steps.length === 0 && (
        <div className="rounded-md border border-dashed border-line-strong px-3 py-3 text-center text-sm text-fg-faint">
          A macro runs its steps in order. Add the first step below.
        </div>
      )}
      {steps.map((step, index) => (
        <div
          key={index}
          onMouseEnter={() => setHovered(index)}
          onMouseLeave={() => setHovered(null)}
          className="flex items-start gap-1.5 rounded-md border bg-surface p-1.5"
        >
          <span className="mt-[5px] w-[18px] shrink-0 text-center text-sm text-fg-faint tabular-nums">{index + 1}</span>
          <div className="min-w-0 flex-1">
            <div className="mb-1 text-sm text-fg-muted">{stepLabel(step.type)}</div>
            <StepEditor step={step} onChange={(next, key) => setStep(index, next, key)} />
          </div>
          <div className={cn("flex shrink-0 flex-col transition-opacity", hovered === index ? "opacity-100" : "opacity-30")}>
            <IconButton label="Move up" size="sm" disabled={index === 0} onClick={() => onChange(move(steps, index, index - 1))}>
              <ArrowUp size={13} />
            </IconButton>
            <IconButton label="Move down" size="sm" disabled={index === steps.length - 1} onClick={() => onChange(move(steps, index, index + 1))}>
              <ArrowDown size={13} />
            </IconButton>
          </div>
          <IconButton label="Remove step" size="sm" tone="danger" onClick={() => onChange(steps.filter((_, i) => i !== index))}>
            <Trash2 size={13} />
          </IconButton>
        </div>
      ))}
      <Menu>
        <MenuTrigger asChild>
          <Button size="sm" className="self-start" icon={<Plus size={13} />}>
            Add step
          </Button>
        </MenuTrigger>
        <MenuContent align="start">
          {STEP_TYPES.map((info) => (
            <MenuItem key={info.type} onSelect={() => onChange([...steps, defaultStep(info.type)])}>
              {info.label}
            </MenuItem>
          ))}
        </MenuContent>
      </Menu>
    </div>
  );
}

/** A labelled row that holds one switch (used for small boolean options next to actions). */
export function InlineSwitch({
  label,
  checked,
  onChange,
  hint,
}: {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  hint?: string;
}) {
  return (
    <label className="flex items-center gap-2 text-fg-muted" title={hint}>
      <Switch checked={checked} onChange={onChange} aria-label={label} />
      <span>{label}</span>
    </label>
  );
}
