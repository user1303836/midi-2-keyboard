import {
  Activity,
  ArrowDown,
  ArrowDownToLine,
  ArrowUp,
  ArrowUpDown,
  FastForward,
  Hourglass,
  MousePointerClick,
  Plus,
  Repeat,
  ToggleLeft,
  X,
  Zap,
} from "lucide-react";
import { useContext, useLayoutEffect, useRef, useState, type ReactNode } from "react";

import type { Action } from "@/bindings/Action";
import type { Behavior } from "@/bindings/Behavior";
import type { HoldTarget } from "@/bindings/HoldTarget";
import type { ShuttleRest } from "@/bindings/ShuttleRest";
import type { Source } from "@/bindings/Source";
import { Button, IconButton } from "@/components/ui/button";
import { NumberInput, Segmented, Switch } from "@/components/ui/inputs";
import { Tooltip } from "@/components/ui/overlays";
import {
  BEHAVIOR_TYPES,
  behaviorTypeInfo,
  defaultBehavior,
  emptyKeys,
  isEmptyAction,
  rangeLimit,
  shuttleAt,
  shuttleRate,
  shuttleRest,
  shuttleRestZone,
  shuttleSides,
  shuttleTravel,
  type BehaviorType,
  type ShuttleBehavior,
  type SourceShape,
} from "@/lib/model";
import { isModifier } from "@/lib/keys";
import { platform } from "@/lib/platform";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";

import { ActionEditor, InlineSwitch, SourceContext, TargetEditor } from "./ActionEditor";
import { useLiveValue } from "./fields";

const ICONS: Record<BehaviorType, ReactNode> = {
  trigger: <Zap size={15} />,
  hold: <ArrowDownToLine size={15} />,
  toggle: <ToggleLeft size={15} />,
  change: <Activity size={15} />,
  step: <ArrowUpDown size={15} />,
  shuttle: <FastForward size={15} />,
  tap_hold: <Hourglass size={15} />,
  multi_tap: <MousePointerClick size={15} />,
  cycle: <Repeat size={15} />,
};

export function BehaviorIcon({ type }: { type: BehaviorType }) {
  return <>{ICONS[type]}</>;
}

/**
 * Where the chooser stood when one of its tiles was clicked (kept outside the component: in
 * the quick add the whole editor is rebuilt for the new behavior).
 */
let clickedAt: number | null = null;

/** The behavior chooser: the ones that suit the source come first and are highlighted. */
export function BehaviorPicker({
  value,
  shape,
  onChange,
}: {
  value: BehaviorType;
  shape: SourceShape;
  onChange: (type: BehaviorType) => void;
}) {
  // Always in the same places (a row for buttons, a row for knobs and wheels, a row for the
  // elaborate button gestures): the ones that suit the source stand out, the rest are dimmed.
  // They used to be sorted by fit, but choosing "Press" for a knob makes it a button, and the
  // tiles then changed places under the pointer.
  const sorted = BEHAVIOR_TYPES;
  // Choosing a behavior can change what is shown above the chooser (a "pressed" range appears
  // for a knob, a dead zone field goes away…). The tile that was just clicked has to stay under
  // the pointer, or the next click lands on its neighbour: scroll by whatever the chooser moved.
  const grid = useRef<HTMLDivElement>(null);
  // What scrolling cannot make up for (things above got shorter while the panel is already
  // scrolled to its top) is held open with a spacer, until the pointer or the focus leaves.
  const [spacer, setSpacer] = useState(0);
  useLayoutEffect(() => {
    const node = grid.current;
    if (!node || clickedAt === null) return;
    let moved = node.getBoundingClientRect().top - clickedAt;
    clickedAt = null;
    if (moved === 0) return;
    for (let parent = node.parentElement; parent; parent = parent.parentElement) {
      const overflow = getComputedStyle(parent).overflowY;
      if (overflow === "auto" || overflow === "scroll") {
        const before = parent.scrollTop;
        parent.scrollTop += moved;
        moved -= parent.scrollTop - before;
        break;
      }
    }
    if (moved < -0.5) setSpacer((height) => height - moved);
  });
  return (
    <div
      ref={grid}
      className="grid grid-cols-3 gap-1.5"
      style={spacer ? { marginTop: spacer } : undefined}
      onMouseLeave={() => setSpacer(0)}
      onBlur={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget)) setSpacer(0);
      }}
    >
      {sorted.map((info) => {
        const selected = info.type === value;
        const fits = info.fits.includes(shape);
        return (
          <Tooltip
            key={info.type}
            side="top"
            content={
              fits || selected
                ? info.hint
                : shape === "continuous"
                  ? `${info.hint}. For a knob or fader this treats it as a switch that is “pressed” in the upper half of its travel (you can move that point).`
                  : shape === "pulse"
                    ? `${info.hint}. This message has no release, so it will not do much here.`
                    : `${info.hint}. Not the usual choice for this kind of control, but it works.`
            }
          >
            <button
              type="button"
              aria-pressed={selected}
              onClick={() => {
                clickedAt = grid.current?.getBoundingClientRect().top ?? null;
                onChange(info.type);
              }}
              className={cn(
                "flex h-[50px] flex-col items-center justify-center gap-1 rounded-lg border text-sm transition-colors",
                selected
                  ? "border-accent bg-accent-soft font-medium text-fg"
                  : "border-line-strong bg-app text-fg-muted hover:border-fg-faint hover:text-fg",
                !fits && !selected && "opacity-60 hover:opacity-100",
              )}
            >
              <span className={selected ? "text-accent" : undefined}>{ICONS[info.type]}</span>
              {info.label}
            </button>
          </Tooltip>
        );
      })}
    </div>
  );
}

/** One labelled action row inside a behavior form. */
function Slot({
  label,
  children,
  onRemove,
}: {
  label: ReactNode;
  children: ReactNode;
  onRemove?: () => void;
}) {
  return (
    <div className="flex items-start gap-2">
      <div className="flex h-[34px] w-[62px] shrink-0 items-center text-fg-muted">{label}</div>
      <div className="min-w-0 flex-1">{children}</div>
      {onRemove && (
        <IconButton label="Remove" size="sm" className="mt-[5px]" onClick={onRemove}>
          <X size={13} />
        </IconButton>
      )}
    </div>
  );
}

function AddSlot({ label, onAdd }: { label: string; onAdd: () => void }) {
  return (
    <Button size="sm" variant="ghost" className="self-start" icon={<Plus size={13} />} onClick={onAdd}>
      {label}
    </Button>
  );
}

function Ms({
  value,
  onChange,
  min = 1,
  max = 60000,
  label,
}: {
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  label: string;
}) {
  return <NumberInput aria-label={label} suffix="ms" className="w-[88px]" min={min} max={max} value={value} onChange={onChange} />;
}

export function BehaviorEditor({
  behavior,
  shape,
  onChange,
  armedSlot = null,
  onCaptured,
  compact = false,
}: {
  behavior: Behavior;
  shape: SourceShape;
  /** The quick add: keep to what the two gestures need, the rest one click away. */
  compact?: boolean;
  onChange: (behavior: Behavior, coalesceKey?: string) => void;
  /** Which action slot (0 = first) should start capturing keys right away (quick-add flow). */
  armedSlot?: number | null;
  /** A slot finished capturing a shortcut. */
  onCaptured?: (slot: number) => void;
}) {
  const settings = useApp((state) => state.config.settings);
  const b = behavior;
  const info = behaviorTypeInfo(b.type);
  const pulse = shape === "pulse";
  // Only empty slots are armed, so walking through the quick-add never overwrites a shortcut.
  const arm = (slot: number, action: Action | null) => ({
    autoCapture: armedSlot === slot && isEmptyAction(action),
    onCaptured: () => onCaptured?.(slot),
  });
  const armTarget = (slot: number, target: HoldTarget) => ({
    autoCapture: armedSlot === slot && target.type === "keys" && target.keys.length === 0,
    onCaptured: () => onCaptured?.(slot),
  });

  return (
    <div className="flex flex-col gap-3">
      <BehaviorPicker value={b.type} shape={shape} onChange={(type) => onChange(defaultBehavior(type, b))} />
      <div className="text-sm text-fg-faint">{info.hint}.</div>

      {b.type === "trigger" && (
        <div className="flex flex-col gap-2.5">
          {b.on_press ? (
            <Slot label={b.on_release ? "Press" : "Send"} onRemove={b.on_release ? () => onChange({ ...b, on_press: null }) : undefined}>
              <ActionEditor
                action={b.on_press}
                {...arm(0, b.on_press)}
                onChange={(on_press, key) => onChange({ ...b, on_press }, key && `press:${key}`)}
              />
            </Slot>
          ) : (
            <AddSlot label="Do something on press" onAdd={() => onChange({ ...b, on_press: emptyKeys() })} />
          )}
          {b.on_release ? (
            <Slot label="Release" onRemove={() => onChange({ ...b, on_release: null })}>
              <ActionEditor
                action={b.on_release}
                onChange={(on_release, key) => onChange({ ...b, on_release }, key && `release:${key}`)}
              />
            </Slot>
          ) : (
            !pulse && <AddSlot label="Also do something on release" onAdd={() => onChange({ ...b, on_release: emptyKeys() })} />
          )}
          {!pulse && (
            <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
              <InlineSwitch
                label="Repeat while held"
                hint="Auto-fire: keep sending the press action for as long as the control stays pressed"
                checked={b.repeat !== null}
                onChange={(on) => onChange({ ...b, repeat: on ? { delay_ms: 400, interval_ms: 100 } : null })}
              />
              {b.repeat && (
                <div className="flex items-center gap-2 text-sm text-fg-muted">
                  after
                  <Ms label="Delay before repeating" min={0} value={b.repeat.delay_ms} onChange={(delay_ms) => onChange({ ...b, repeat: { ...b.repeat!, delay_ms } }, "rdelay")} />
                  every
                  <Ms label="Repeat interval" min={5} value={b.repeat.interval_ms} onChange={(interval_ms) => onChange({ ...b, repeat: { ...b.repeat!, interval_ms } }, "rint")} />
                </div>
              )}
            </div>
          )}
        </div>
      )}

      {b.type === "hold" && (
        <div className="flex flex-col gap-2.5">
          <Slot label="Hold">
            <TargetEditor
              target={b.target}
              {...armTarget(0, b.target)}
              onChange={(target, key) => onChange({ ...b, target }, key && `target:${key}`)}
            />
          </Slot>
          {b.target.type === "keys" && (
            <InlineSwitch
              label="Auto-repeat like a real keyboard"
              hint={`While held, the key repeats after ${settings.repeat_delay_ms} ms, every ${settings.repeat_interval_ms} ms (change in Settings)`}
              checked={b.auto_repeat}
              onChange={(auto_repeat) => onChange({ ...b, auto_repeat })}
            />
          )}
          {pulse && (
            <div className="rounded-md bg-warn-soft px-2.5 py-1.5 text-sm text-warn">
              This message has no “release”, so there is nothing to hold. Try Press or Toggle.
            </div>
          )}
          {platform() === "macos" && b.target.type === "keys" && b.target.keys.length > 0 && b.target.keys.every(isModifier) && (
            <div className="text-sm text-fg-faint">
              On macOS a modifier held this way applies to keys and clicks sent by MIDI 2 Keyboard, not to
              your own keyboard or mouse.
            </div>
          )}
        </div>
      )}

      {b.type === "toggle" && (
        <Slot label="Latch">
          <TargetEditor
            target={b.target}
            {...armTarget(0, b.target)}
            onChange={(target, key) => onChange({ ...b, target }, key && `target:${key}`)}
          />
        </Slot>
      )}

      {b.type === "change" && (
        <div className="flex flex-col gap-2.5">
          <Slot label="Send">
            <ActionEditor
              action={b.action}
              {...arm(0, b.action)}
              onChange={(action, key) => onChange({ ...b, action }, key && `action:${key}`)}
            />
          </Slot>
          {shape === "continuous" && (
            <>
              <InlineSwitch
                label="Ignore jitter"
                hint="A worn or noisy knob flutters by a step or two while nobody touches it, and every flutter would fire. Turn this on to react only to real movement."
                checked={b.min_change > 1}
                onChange={(on) => onChange({ ...b, min_change: on ? 3 : 0 })}
              />
              {b.min_change > 1 && (
                <div className="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
                  React only when the value has moved by at least
                  <NumberInput
                    aria-label="Smallest change that counts"
                    className="w-[64px]"
                    min={2}
                    max={8191}
                    value={b.min_change}
                    onChange={(min_change) => onChange({ ...b, min_change }, "minchange")}
                  />
                </div>
              )}
            </>
          )}
        </div>
      )}

      {b.type === "step" && (
        <div className="flex flex-col gap-2.5">
          <StepSlot
            label={<><ArrowUp size={14} /> Up</>}
            action={b.up}
            {...arm(0, b.up)}
            onChange={(up, key) => onChange({ ...b, up }, key && `up:${key}`)}
          />
          <StepSlot
            label={<><ArrowDown size={14} /> Down</>}
            action={b.down}
            {...arm(1, b.down)}
            onChange={(down, key) => onChange({ ...b, down }, key && `down:${key}`)}
          />
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2 text-sm text-fg-muted">
            <span className="flex items-center gap-2">
              One action per
              <NumberInput aria-label="Step size" className="w-[60px]" min={1} max={16383} value={b.step} onChange={(step) => onChange({ ...b, step }, "step")} />
              {shape === "relative" ? "ticks" : "units of movement"}
            </span>
            <InlineSwitch label="Reverse direction" checked={b.invert} onChange={(invert) => onChange({ ...b, invert })} />
          </div>
          <InlineSwitch
            label="Do something else when it is turned fast"
            hint="Fine steps for a slow turn, a coarser action for a quick spin (e.g. Right for one frame, Shift+Right for ten)"
            checked={b.fast !== null}
            onChange={(on) => onChange({ ...b, fast: on ? { ticks: 3, up: emptyKeys(), down: emptyKeys() } : null })}
          />
          {b.fast && (
            <div className="flex flex-col gap-2.5 rounded-lg border bg-surface p-2.5">
              <StepSlot
                label={<><ArrowUp size={14} /> Fast</>}
                action={b.fast.up}
                onChange={(up, key) => onChange({ ...b, fast: { ...b.fast!, up } }, key && `fup:${key}`)}
              />
              <StepSlot
                label={<><ArrowDown size={14} /> Fast</>}
                action={b.fast.down}
                onChange={(down, key) => onChange({ ...b, fast: { ...b.fast!, down } }, key && `fdown:${key}`)}
              />
              <div className="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
                Counts as fast when one message jumps
                <NumberInput
                  aria-label="Steps that count as fast"
                  className="w-[60px]"
                  min={2}
                  max={127}
                  value={b.fast.ticks}
                  onChange={(ticks) => onChange({ ...b, fast: { ...b.fast!, ticks } }, "fticks")}
                />
                steps or more: the fast action then fires once, instead of the normal one per step.
              </div>
              <div className="text-sm leading-snug text-fg-faint">
                A direction left empty keeps its normal action.
                {shape === "relative" &&
                  " An encoder only ever jumps several steps at once when its acceleration is switched on (on the controller)."}
              </div>
            </div>
          )}
          {shape === "continuous" && <EndStopNote />}
          <div className="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
            <span className="whitespace-nowrap">Fire at most</span>
            <NumberInput aria-label="Maximum actions per message" className="w-[60px]" min={0} max={1000} value={b.max_burst} onChange={(max_burst) => onChange({ ...b, max_burst }, "burst")} />
            <span className="whitespace-nowrap">times per message</span>
            <span className="text-fg-faint" title="A fast spin or a jump of the control can ask for dozens of steps at once. This caps it. 0 = no limit.">
              (tames sudden jumps)
            </span>
          </div>
        </div>
      )}

      {b.type === "shuttle" && <ShuttleForm b={b} shape={shape} compact={compact} onChange={onChange} arm={arm} />}

      {b.type === "tap_hold" && (
        <div className="flex flex-col gap-2.5">
          {b.tap ? (
            <Slot label="Tap" onRemove={() => onChange({ ...b, tap: null })}>
              <ActionEditor
                action={b.tap}
                {...arm(0, b.tap)}
                onChange={(tap, key) => onChange({ ...b, tap }, key && `tap:${key}`)}
              />
            </Slot>
          ) : (
            <AddSlot label="Do something on a quick tap" onAdd={() => onChange({ ...b, tap: emptyKeys() })} />
          )}
          <Slot label="Hold">
            <div className="flex flex-col gap-2">
              <Segmented
                className="self-start"
                value={b.hold_target ? "target" : "action"}
                options={[
                  { value: "action", label: "Fire once", title: "Run an action the moment the hold time is reached" },
                  { value: "target", label: "Keep held", title: "Hold keys down from then until release (dual-role key)" },
                ]}
                onChange={(mode) =>
                  onChange(
                    mode === "target"
                      ? { ...b, hold: null, hold_target: { type: "keys", keys: b.hold?.type === "keys" ? b.hold.keys : [] } }
                      : {
                          ...b,
                          hold_target: null,
                          hold: { type: "keys", keys: b.hold_target?.type === "keys" ? b.hold_target.keys : [], tap_ms: null },
                        },
                  )
                }
              />
              {b.hold_target ? (
                <TargetEditor target={b.hold_target} {...armTarget(1, b.hold_target)} onChange={(hold_target, key) => onChange({ ...b, hold_target }, key && `ht:${key}`)} />
              ) : (
                <ActionEditor action={b.hold ?? emptyKeys()} {...arm(1, b.hold)} onChange={(hold, key) => onChange({ ...b, hold }, key && `hold:${key}`)} />
              )}
            </div>
          </Slot>
          <div className="flex items-center gap-2 text-sm text-fg-muted">
            Counts as a hold after
            <Ms label="Hold time" min={50} max={10000} value={b.hold_ms} onChange={(hold_ms) => onChange({ ...b, hold_ms }, "holdms")} />
          </div>
        </div>
      )}

      {b.type === "multi_tap" && (
        <div className="flex flex-col gap-2.5">
          {(
            [
              ["single", "Single"],
              ["double", "Double"],
              ["triple", "Triple"],
            ] as const
          ).map(([field, label], index) => {
            const action = b[field];
            return action ? (
              <Slot key={field} label={label} onRemove={() => onChange({ ...b, [field]: null })}>
                <ActionEditor
                  action={action}
                  {...arm(index, action)}
                  onChange={(next, key) => onChange({ ...b, [field]: next }, key && `${field}:${key}`)}
                />
              </Slot>
            ) : (
              <AddSlot key={field} label={`${label} press`} onAdd={() => onChange({ ...b, [field]: emptyKeys() })} />
            );
          })}
          {b.hold ? (
            <Slot label="Long" onRemove={() => onChange({ ...b, hold: null })}>
              <ActionEditor
                action={b.hold}
                {...arm(3, b.hold)}
                onChange={(hold, key) => onChange({ ...b, hold }, key && `hold:${key}`)}
              />
            </Slot>
          ) : (
            <AddSlot label="Long press" onAdd={() => onChange({ ...b, hold: emptyKeys() })} />
          )}
          <div className="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
            Presses count together when they are within
            <Ms label="Multi-tap window" min={50} max={5000} value={b.window_ms} onChange={(window_ms) => onChange({ ...b, window_ms }, "window")} />
          </div>
          {b.hold && (
            <div className="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
              Holding for
              <Ms label="Long press time" min={100} max={10000} value={b.hold_ms} onChange={(hold_ms) => onChange({ ...b, hold_ms }, "holdms")} />
              is a long press (and never also a single press)
            </div>
          )}
          <div className="text-sm text-fg-faint">
            A single press has to wait for that window to be sure a second one is not coming.
          </div>
        </div>
      )}

      {b.type === "cycle" && (
        <div className="flex flex-col gap-2.5">
          {b.actions.map((action, index) => (
            <Slot
              key={index}
              label={`Press ${index + 1}`}
              onRemove={b.actions.length > 1 ? () => onChange({ ...b, actions: b.actions.filter((_, i) => i !== index) }) : undefined}
            >
              <ActionEditor
                action={action}
                {...arm(index, action)}
                onChange={(next, key) =>
                  onChange({ ...b, actions: b.actions.map((a, i) => (i === index ? next : a)) }, key && `c${index}:${key}`)
                }
              />
            </Slot>
          ))}
          <AddSlot label="Add another" onAdd={() => onChange({ ...b, actions: [...b.actions, emptyKeys()] })} />
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <InlineSwitch
              label="Start over after a pause"
              checked={b.reset_after_ms !== null}
              onChange={(on) => onChange({ ...b, reset_after_ms: on ? 2000 : null })}
            />
            {b.reset_after_ms !== null && (
              <span className="flex items-center gap-2 text-sm text-fg-muted">
                of
                <Ms label="Idle time before restarting" min={100} max={600000} value={b.reset_after_ms} onChange={(reset_after_ms) => onChange({ ...b, reset_after_ms }, "reset")} />
              </span>
            )}
          </div>
        </div>
      )}
    </div>
  );
}

/**
 * Up / Down on a control with end stops (or an endless encoder left in its absolute mode, which
 * many controllers ship with): at 0 and at the top it sends nothing more, and the steps simply
 * stop — which looks like the mapping broke. Said here, and said louder the moment it happens.
 */
function EndStopNote() {
  const source = useContext(SourceContext);
  const live = useLiveValue(source ?? NO_SOURCE);
  const limit = source ? rangeLimit(source.event) : null;
  if (!source || limit === null) return null;
  const atEnd = live !== null && (live <= 0 || live >= limit);
  const cure =
    source.event.type === "control_change"
      ? "An encoder in a relative mode never runs out: set that on the controller, then choose “Endless encoder” above."
      : "An encoder sending relative control changes never runs out.";
  return (
    <div
      data-end-stop={atEnd ? "now" : "note"}
      className={cn("rounded-md px-2.5 py-1.5 text-sm leading-snug", atEnd ? "bg-warn-soft text-warn" : "bg-app text-fg-faint")}
    >
      {atEnd
        ? `It is at the end of its travel now (${live}): turning further sends nothing, so the steps stop. `
        : `At the ends of its travel (0 and ${limit}) it sends nothing more, so the steps stop there. `}
      {cure}
    </div>
  );
}

/** "2.5", "33": firings per second, short. */
function perSecond(rate: number): string {
  return rate < 10 ? String(Math.round(rate * 10) / 10) : String(Math.round(rate));
}

/** What a pace means for the action it repeats, where a number says more than "times a second". */
function paceUnit(action: Action | null): { per: number; unit: string } {
  if (action?.type === "mouse_move" && !action.absolute) {
    const distance = Math.hypot(action.x, action.y);
    if (distance > 0) return { per: distance, unit: "pixels a second" };
  }
  if (action?.type === "mouse_scroll") {
    const notches = Math.hypot(action.dx, action.dy);
    if (notches > 0) return { per: notches, unit: "wheel notches a second" };
  }
  return { per: 1, unit: "times a second" };
}

const NO_SOURCE: Source = {
  device: null,
  channel: null,
  event: { type: "control_change", controller: 0, range: { min: 0, max: 127 }, encoding: "absolute" },
  hysteresis: 0,
  debounce_ms: 0,
};

/**
 * Shuttle: the control's position sets the pace. The form reads like a sentence per line, and
 * the strip at the bottom shows the control live: where it is, and how fast that is.
 */
function ShuttleForm({
  b,
  shape,
  compact,
  onChange,
  arm,
}: {
  b: ShuttleBehavior;
  shape: SourceShape;
  /** Quick add: the two actions first, the fine-tuning one click away. */
  compact: boolean;
  onChange: (behavior: Behavior, coalesceKey?: string) => void;
  arm: (slot: number, action: Action | null) => { autoCapture: boolean; onCaptured: () => void };
}) {
  const [open, setOpen] = useState(!compact);
  const source = useContext(SourceContext);
  const event = source?.event ?? null;
  const live = useLiveValue(source ?? NO_SOURCE);
  const rest = shuttleRest(b, event);
  const sides = shuttleSides(b, event);
  const both = sides.up && sides.down;
  const travel = shuttleTravel(event);
  const zone = shuttleRestZone(b, event);
  const now = live === null ? null : shuttleAt(b, event, live);
  const hit = event?.type === "note" || event?.type === "chord";

  const unit = paceUnit((sides.up ? b.up : null) ?? (sides.down ? b.down : null));
  const span = Math.max(1, travel.max - travel.min);
  const at = (value: number) => `${(Math.min(Math.max(value - travel.min, 0), span) / span) * 100}%`;
  // Values read as they do everywhere else: pitch bend counts from its middle (+8191 … −8192).
  const bend = event?.type === "pitch_bend";
  const shown = (value: number) => (bend ? `${value > 8192 ? "+" : ""}${value - 8192}` : String(value));
  const restText =
    b.dead_zone === 0
      ? `only ${shown(zone.rest)} itself`
      : rest === "low"
        ? `values up to ${shown(zone.max)}`
        : rest === "high"
          ? `values from ${shown(zone.min)}`
          : bend
            ? `${shown(zone.min)} to ${shown(zone.max)}`
            : `values ${zone.min}–${zone.max}`;

  const restRow = (
    <div className="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
      <span className="shrink-0">{hit ? "The softest hit is" : "It rests"}</span>
      <Segmented<ShuttleRest>
        size="sm"
        value={rest}
        options={[
          { value: "center", label: "in the middle", title: "A pitch wheel, a joystick axis, a shuttle ring: one action above the middle, another below" },
          { value: "low", label: "at the bottom", title: "A fader, a pedal, pressure, how hard a pad is hit: the higher, the faster" },
          { value: "high", label: "at the top", title: "A control that reads backwards: the lower, the faster" },
        ]}
        onChange={(next) => onChange({ ...b, rest: next })}
      />
    </div>
  );

  return (
    <div className="flex flex-col gap-2.5">
      {(shape === "relative" || shape === "pulse") && (
        <div className="rounded-md bg-warn-soft px-2.5 py-1.5 text-sm text-warn">
          {shape === "relative"
            ? "An endless encoder sends steps, not a position, so there is nothing to set the pace. Try Up / Down."
            : "This message has no position, so there is nothing to set the pace. Try Press."}
        </div>
      )}
      {open && restRow}
      {sides.up && (
        <StepSlot
          label={both ? <><ArrowUp size={14} /> Up</> : "Repeat"}
          action={b.up}
          {...arm(0, b.up)}
          onChange={(up, key) => onChange({ ...b, up }, key && `up:${key}`)}
        />
      )}
      {sides.down && (
        <StepSlot
          label={both ? <><ArrowDown size={14} /> Down</> : "Repeat"}
          action={b.down}
          {...arm(both ? 1 : 0, b.down)}
          onChange={(down, key) => onChange({ ...b, down }, key && `down:${key}`)}
        />
      )}
      {open ? (
        <>
          <div className="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
            <span className="whitespace-nowrap">Every</span>
            <NumberInput
              aria-label="Pace near rest"
              suffix="ms"
              className="w-[80px]"
              min={1}
              max={600000}
              value={b.slow_ms}
              onChange={(slow_ms) => onChange({ ...b, slow_ms, fast_ms: Math.min(b.fast_ms, slow_ms) }, "slow")}
            />
            <span className="whitespace-nowrap">{hit ? "hit softly, every" : "near rest, every"}</span>
            <NumberInput
              aria-label="Pace at the end of the travel"
              suffix="ms"
              className="w-[80px]"
              min={1}
              max={60000}
              value={b.fast_ms}
              onChange={(fast_ms) => onChange({ ...b, fast_ms, slow_ms: Math.max(b.slow_ms, fast_ms) }, "fast")}
            />
            <span className="whitespace-nowrap">{hit ? "hit hard" : "at the end"}</span>
          </div>
          <div className="text-sm text-fg-faint" data-shuttle-pace>
            That is {perSecond(shuttleRate(b, 0) * unit.per)} to {perSecond(shuttleRate(b, 1) * unit.per)} {unit.unit}.
          </div>
          <label
            className="flex flex-wrap items-center gap-x-2 gap-y-0.5 text-fg-muted"
            title="The pace grows with the square of the distance: fine control near rest, where it matters, and full speed only at the very end (what game controllers do for a pointer)"
          >
            <Switch
              checked={b.curve === "squared"}
              onChange={(on) => onChange({ ...b, curve: on ? "squared" : "linear" })}
              aria-label="Gentle near rest"
            />
            <span>Gentle near rest</span>
            <span className="text-sm text-fg-faint">slow for small pushes, fast only near the end</span>
          </label>
          <div className="flex flex-wrap items-center gap-2 text-sm text-fg-muted">
            <span className="whitespace-nowrap">Rest zone:</span>
            <NumberInput
              aria-label="Dead zone"
              suffix="%"
              className="w-[72px]"
              min={0}
              max={90}
              value={b.dead_zone}
              onChange={(dead_zone) => onChange({ ...b, dead_zone }, "dead")}
            />
            <span>
              {hit ? "of the velocities" : "of the travel"} <span className="text-fg-faint">({restText})</span>
            </span>
          </div>
        </>
      ) : (
        <div className="text-sm leading-snug text-fg-faint" data-shuttle-pace>
          {rest === "center" ? "Rests in the middle" : rest === "low" ? "Rests at the bottom" : "Rests at the top"}; {perSecond(shuttleRate(b, 0) * unit.per)} to{" "}
          {perSecond(shuttleRate(b, 1) * unit.per)} {unit.unit}.{" "}
          <button type="button" className="text-fg-muted underline underline-offset-2 hover:text-fg" onClick={() => setOpen(true)}>
            Adjust
          </button>
        </div>
      )}
      <div>
        <div
          className="relative h-[22px] overflow-hidden rounded-md border bg-app"
          role="img"
          aria-label="Where the control is, and the part of its travel that counts as rest"
        >
          {sides.down && (
            <span
              className="absolute inset-y-0 bg-linear-to-l from-transparent to-midi-soft"
              style={{ left: 0, width: at(zone.min) }}
            />
          )}
          {sides.up && (
            <span
              className="absolute inset-y-0 bg-linear-to-r from-transparent to-midi-soft"
              style={{ left: at(zone.max), right: 0 }}
            />
          )}
          <span
            className="absolute inset-y-0 border-x border-line-strong bg-hover"
            style={{ left: at(zone.min), width: `max(3px, calc(${at(zone.max)} - ${at(zone.min)}))` }}
            title={`Rest: ${restText}`}
          />
          {live !== null && (
            <span
              className="pointer-events-none absolute inset-y-0 w-[2px] bg-midi shadow-[0_0_6px_var(--midi)]"
              style={{ left: `calc(${at(live)} - 1px)` }}
            />
          )}
        </div>
        <div className="mt-1 text-sm text-fg-faint" data-shuttle-live>
          {live === null
            ? hit
              ? "Hit the pad to see how fast that is."
              : "Move the control to see how fast that is."
            : now
              ? `At ${shown(live)}: ${both ? (now.side === "up" ? "up, " : "down, ") : ""}${perSecond(now.rate * paceUnit(now.side === "up" ? b.up : b.down).per)} ${paceUnit(now.side === "up" ? b.up : b.down).unit}.`
              : `At ${shown(live)}: at rest.`}
        </div>
      </div>
    </div>
  );
}

function StepSlot({
  label,
  action,
  onChange,
  autoCapture,
  onCaptured,
}: {
  label: ReactNode;
  action: Action | null;
  onChange: (action: Action | null, coalesceKey?: string) => void;
  autoCapture?: boolean;
  onCaptured?: () => void;
}) {
  if (!action) {
    return (
      <Slot label={<span className="inline-flex items-center gap-1">{label}</span>}>
        <Button size="sm" variant="ghost" className="mt-[5px]" icon={<Plus size={13} />} onClick={() => onChange(emptyKeys())}>
          Add action
        </Button>
      </Slot>
    );
  }
  return (
    <Slot label={<span className="inline-flex items-center gap-1">{label}</span>} onRemove={() => onChange(null)}>
      <ActionEditor action={action} autoCapture={autoCapture} onCaptured={onCaptured} onChange={onChange} />
    </Slot>
  );
}
