// Rate limiting and conditions: the two "advanced" groups of a mapping.

import { Plus, Trash2 } from "lucide-react";

import type { Condition } from "@/bindings/Condition";
import type { RateLimit } from "@/bindings/RateLimit";
import { Button, IconButton } from "@/components/ui/button";
import { NumberInput, Segmented, Switch, TextInput } from "@/components/ui/inputs";
import { Menu, MenuContent, MenuItem, MenuTrigger } from "@/components/ui/overlays";
import { CONDITION_TYPES, defaultCondition, defaultRateLimit, knownLayers } from "@/lib/model";
import { useApp } from "@/lib/store";
import { cn, formatMs } from "@/lib/utils";

import { ChannelSelect, NoteInput, RangeEditor } from "./fields";

const PRESETS = [50, 100, 250, 500, 1000, 2000];

/** A tiny timeline showing what a rate limit does to a burst of input. */
function RateDiagram({ limit }: { limit: RateLimit }) {
  // 26 input ticks over the diagram, in two bursts; the interval spans 6 ticks.
  const inputs = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 20, 21, 22, 23];
  const span = 6;
  const outputs: number[] = [];
  if (limit.mode === "throttle") {
    let windowEnd = -Infinity;
    let pending = false;
    for (let t = 0; t <= 34; t++) {
      if (t === windowEnd && pending && limit.trailing) {
        outputs.push(t);
        windowEnd = t + span;
        pending = false;
      }
      if (inputs.includes(t)) {
        if (t >= windowEnd) {
          outputs.push(t);
          windowEnd = t + span;
          pending = false;
        } else {
          pending = true;
        }
      }
    }
  } else {
    inputs.forEach((t, index) => {
      const next = inputs[index + 1];
      if (next === undefined || next - t > span) outputs.push(t + span);
    });
  }
  const x = (t: number) => 6 + t * 7.4;
  return (
    <svg viewBox="0 0 270 44" className="h-[44px] w-full" role="img" aria-label="How the rate limit thins out input">
      <text x="0" y="13" className="fill-fg-faint text-[8px]">in</text>
      <text x="0" y="36" className="fill-fg-faint text-[8px]">out</text>
      <line x1="16" y1="10" x2="268" y2="10" className="stroke-line" />
      <line x1="16" y1="33" x2="268" y2="33" className="stroke-line" />
      {inputs.map((t) => (
        <rect key={`i${t}`} x={x(t) + 14} y="4" width="2.4" height="12" rx="1.2" className="fill-midi" />
      ))}
      {outputs.map((t, index) => (
        <rect key={`o${index}`} x={x(t) + 13} y="26" width="4.4" height="14" rx="2" className="fill-key" />
      ))}
    </svg>
  );
}

export function RateLimitEditor({
  value,
  onChange,
}: {
  value: RateLimit | null;
  onChange: (value: RateLimit | null, coalesceKey?: string) => void;
}) {
  return (
    <div className="flex flex-col gap-2.5">
      <label className="flex items-center gap-2">
        <Switch
          aria-label="Limit how often this fires"
          checked={value !== null}
          onChange={(on) => onChange(on ? defaultRateLimit() : null)}
        />
        <span>Limit how often this fires</span>
      </label>
      {value && (
        <div className="flex flex-col gap-2.5 rounded-lg border bg-surface p-3">
          <Segmented
            className="w-full"
            value={value.mode}
            options={[
              { value: "throttle", label: "At most once every…", title: "Fire right away, then ignore input for a while" },
              { value: "debounce", label: "Once it settles for…", title: "Wait until the input has been quiet, then fire once" },
            ]}
            onChange={(mode) => onChange({ ...value, mode })}
          />
          <div className="flex flex-wrap items-center gap-2">
            <NumberInput
              aria-label="Interval"
              suffix="ms"
              className="w-[96px]"
              min={1}
              max={3600000}
              value={value.interval_ms}
              onChange={(interval_ms) => onChange({ ...value, interval_ms }, "interval")}
            />
            <div className="flex flex-wrap gap-1">
              {PRESETS.map((ms) => (
                <button
                  key={ms}
                  type="button"
                  onClick={() => onChange({ ...value, interval_ms: ms })}
                  className={cn(
                    "h-[22px] rounded-md border px-1.5 text-sm transition-colors",
                    value.interval_ms === ms
                      ? "border-accent-line bg-accent-soft text-accent"
                      : "border-line-strong text-fg-muted hover:text-fg",
                  )}
                >
                  {formatMs(ms)}
                </button>
              ))}
            </div>
          </div>
          {value.mode === "throttle" && (
            <label className="flex items-center gap-2 text-fg-muted">
              <Switch
                aria-label="Also fire once at the end"
                checked={value.trailing}
                onChange={(trailing) => onChange({ ...value, trailing })}
              />
              <span>Also fire once at the end, so the last movement is never lost</span>
            </label>
          )}
          <RateDiagram limit={value} />
          <div className="text-sm text-fg-faint">
            {value.mode === "throttle"
              ? `Fires immediately, then stays quiet for ${formatMs(value.interval_ms)} no matter how much input arrives${value.trailing ? "; if more input came in meanwhile, it fires once more when that time is up." : "."}`
              : `Waits until nothing has arrived for ${formatMs(value.interval_ms)}, then fires once.`}
          </div>
        </div>
      )}
    </div>
  );
}

function ConditionRow({
  condition,
  onChange,
  onRemove,
}: {
  condition: Condition;
  onChange: (condition: Condition, coalesceKey?: string) => void;
  onRemove: () => void;
}) {
  const layers = useApp((state) => knownLayers(state.config));
  const recent = useApp((state) => state.engine.foreground);
  return (
    <div className="flex items-start gap-2 rounded-lg border bg-surface p-2.5">
      <div className="flex min-w-0 flex-1 flex-col gap-2">
        {condition.type === "app" && (
          <>
            <div className="flex items-center gap-2">
              <Segmented
                size="sm"
                value={condition.negate ? "not" : "is"}
                options={[
                  { value: "is", label: "While in" },
                  { value: "not", label: "While not in" },
                ]}
                onChange={(mode) => onChange({ ...condition, negate: mode === "not" })}
              />
              <span className="text-sm text-fg-faint">the app…</span>
            </div>
            <TextInput
              aria-label="Process name"
              placeholder={recent?.process ? `Process, e.g. ${recent.process}` : "Process, e.g. Photoshop.exe"}
              value={condition.app.process}
              onChange={(process) => onChange({ ...condition, app: { ...condition.app, process } }, "process")}
            />
            <TextInput
              aria-label="Window title"
              placeholder="Window title contains… (optional, * and ? allowed)"
              value={condition.app.title}
              onChange={(title) => onChange({ ...condition, app: { ...condition.app, title } }, "title")}
            />
          </>
        )}
        {condition.type === "layer" && (
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-fg-muted">Layer</span>
            <TextInput
              aria-label="Layer name"
              className="w-[130px]"
              list="m2k-layer-names-cond"
              value={condition.layer}
              onChange={(layer) => onChange({ ...condition, layer }, "layer")}
            />
            <datalist id="m2k-layer-names-cond">
              {layers.map((layer) => (
                <option key={layer} value={layer} />
              ))}
            </datalist>
            <Segmented
              size="sm"
              value={condition.active ? "on" : "off"}
              options={[
                { value: "on", label: "is on" },
                { value: "off", label: "is off" },
              ]}
              onChange={(mode) => onChange({ ...condition, active: mode === "on" })}
            />
          </div>
        )}
        {condition.type === "note_held" && (
          <div className="flex flex-wrap items-center gap-2">
            <span className="text-fg-muted">Note</span>
            <NoteInput value={condition.note} onChange={(note) => onChange({ ...condition, note }, "note")} />
            <Segmented
              size="sm"
              value={condition.held ? "held" : "free"}
              options={[
                { value: "held", label: "is held" },
                { value: "free", label: "is not held" },
              ]}
              onChange={(mode) => onChange({ ...condition, held: mode === "held" })}
            />
            <ChannelSelect value={condition.channel} onChange={(channel) => onChange({ ...condition, channel })} />
          </div>
        )}
        {condition.type === "control_value" && (
          <>
            <div className="flex flex-wrap items-center gap-2">
              <span className="text-fg-muted">CC</span>
              <NumberInput
                aria-label="Controller"
                className="w-[64px]"
                min={0}
                max={127}
                value={condition.controller}
                onChange={(controller) => onChange({ ...condition, controller }, "cc")}
              />
              <span className="text-fg-muted">is between</span>
              <ChannelSelect value={condition.channel} onChange={(channel) => onChange({ ...condition, channel })} />
            </div>
            <RangeEditor limit={127} value={condition.range} onChange={(range) => onChange({ ...condition, range }, "range")} />
          </>
        )}
      </div>
      <IconButton label="Remove condition" size="sm" tone="danger" onClick={onRemove}>
        <Trash2 size={13} />
      </IconButton>
    </div>
  );
}

export function ConditionsEditor({
  value,
  onChange,
}: {
  value: Condition[];
  onChange: (value: Condition[], coalesceKey?: string) => void;
}) {
  return (
    <div className="flex flex-col gap-2">
      {value.length === 0 && (
        <div className="text-sm text-fg-faint">
          Add a condition to make this mapping work only in a certain app, while a layer is on, or
          while another pad is held down.
        </div>
      )}
      {value.length > 1 && <div className="text-sm text-fg-faint">All of these must be true:</div>}
      {value.map((condition, index) => (
        <ConditionRow
          key={index}
          condition={condition}
          onChange={(next, key) =>
            onChange(value.map((c, i) => (i === index ? next : c)), key && `cond${index}:${key}`)
          }
          onRemove={() => onChange(value.filter((_, i) => i !== index))}
        />
      ))}
      <Menu>
        <MenuTrigger asChild>
          <Button size="sm" className="self-start" icon={<Plus size={13} />}>
            Add condition
          </Button>
        </MenuTrigger>
        <MenuContent align="start" className="w-[280px]">
          {CONDITION_TYPES.map((info) => (
            <MenuItem key={info.type} className="h-auto py-1.5" onSelect={() => onChange([...value, defaultCondition(info.type)])}>
              <div>
                <div>{info.label}</div>
                <div className="text-sm text-fg-faint">{info.hint}</div>
              </div>
            </MenuItem>
          ))}
        </MenuContent>
      </Menu>
    </div>
  );
}
