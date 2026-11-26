import { Select as RSelect, Slider, Switch as RSwitch } from "radix-ui";
import { Check, ChevronDown } from "lucide-react";
import {
  useEffect,
  useRef,
  useState,
  type InputHTMLAttributes,
  type ReactNode,
  type TextareaHTMLAttributes,
} from "react";

import { clamp, cn } from "@/lib/utils";

const FIELD =
  "h-[28px] w-full rounded-md border border-line-strong bg-app px-2 text-base text-fg outline-hidden transition-colors " +
  "placeholder:text-fg-faint hover:border-fg-faint focus:border-accent focus:ring-2 focus:ring-accent-soft " +
  "disabled:opacity-50 disabled:hover:border-line-strong";

// ---------------------------------------------------------------------------------------------
// Text
// ---------------------------------------------------------------------------------------------

export interface TextInputProps extends Omit<InputHTMLAttributes<HTMLInputElement>, "onChange" | "value"> {
  value: string;
  onChange: (value: string) => void;
  mono?: boolean;
  invalid?: boolean;
}

export function TextInput({ value, onChange, className, mono, invalid, ...props }: TextInputProps) {
  return (
    <input
      type="text"
      spellCheck={false}
      autoComplete="off"
      value={value}
      onChange={(event) => onChange(event.target.value)}
      className={cn(FIELD, mono && "font-mono text-sm", invalid && "border-danger focus:border-danger", className)}
      {...props}
    />
  );
}

export interface TextAreaProps extends Omit<TextareaHTMLAttributes<HTMLTextAreaElement>, "onChange" | "value"> {
  value: string;
  onChange: (value: string) => void;
  mono?: boolean;
}

export function TextArea({ value, onChange, className, mono, rows = 3, ...props }: TextAreaProps) {
  return (
    <textarea
      spellCheck={false}
      rows={rows}
      value={value}
      onChange={(event) => onChange(event.target.value)}
      className={cn(FIELD, "h-auto resize-y py-1.5 leading-snug", mono && "font-mono text-sm", className)}
      {...props}
    />
  );
}

// ---------------------------------------------------------------------------------------------
// Number
// ---------------------------------------------------------------------------------------------

export interface NumberInputProps {
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  /** Unit shown inside the field, e.g. "ms". */
  suffix?: string;
  /** Formats the committed value for display (e.g. note names). Parsing still expects a number. */
  className?: string;
  disabled?: boolean;
  "aria-label"?: string;
  id?: string;
}

/**
 * A numeric field that only commits valid, clamped values: type freely, commit with Enter / blur,
 * step with the arrow keys (Shift = ×10) or the mouse wheel while focused.
 */
export function NumberInput({
  value,
  onChange,
  min = -Infinity,
  max = Infinity,
  step = 1,
  suffix,
  className,
  disabled,
  id,
  ...aria
}: NumberInputProps) {
  const [draft, setDraft] = useState<string | null>(null);
  const ref = useRef<HTMLInputElement>(null);

  const commit = (text: string) => {
    setDraft(null);
    const parsed = Number(text.replace(",", "."));
    if (text.trim() === "" || !Number.isFinite(parsed)) return;
    const next = clamp(Math.round(parsed / step) * step, min, max);
    if (next !== value) onChange(next);
  };

  const nudge = (delta: number) => {
    const base = draft !== null && Number.isFinite(Number(draft)) ? Number(draft) : value;
    const next = clamp(base + delta, min, max);
    setDraft(null);
    if (next !== value) onChange(next);
  };

  // Wheel stepping needs a non-passive listener to be able to prevent page scroll.
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      if (document.activeElement !== element) return;
      event.preventDefault();
      nudge((event.deltaY < 0 ? 1 : -1) * step * (event.shiftKey ? 10 : 1));
    };
    element.addEventListener("wheel", onWheel, { passive: false });
    return () => element.removeEventListener("wheel", onWheel);
  });

  return (
    <div className={cn("relative", className ?? "w-[76px]")}>
      <input
        ref={ref}
        id={id}
        type="text"
        inputMode="numeric"
        disabled={disabled}
        value={draft ?? String(value)}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={(event) => commit(event.target.value)}
        onFocus={(event) => event.target.select()}
        onKeyDown={(event) => {
          if (event.key === "Enter") {
            commit(event.currentTarget.value);
            event.currentTarget.select();
          } else if (event.key === "Escape") {
            setDraft(null);
            event.currentTarget.blur();
          } else if (event.key === "ArrowUp") {
            event.preventDefault();
            nudge(step * (event.shiftKey ? 10 : 1));
          } else if (event.key === "ArrowDown") {
            event.preventDefault();
            nudge(-step * (event.shiftKey ? 10 : 1));
          }
        }}
        className={cn(FIELD, "tabular-nums", suffix && "pr-8")}
        {...aria}
      />
      {suffix && (
        <span className="pointer-events-none absolute top-1/2 right-2 -translate-y-1/2 text-sm text-fg-faint">
          {suffix}
        </span>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Select
// ---------------------------------------------------------------------------------------------

export interface SelectOption<T extends string> {
  value: T;
  label: ReactNode;
  hint?: string;
  icon?: ReactNode;
  disabled?: boolean;
  group?: string;
}

export function Select<T extends string>({
  value,
  onChange,
  options,
  className,
  placeholder,
  disabled,
  "aria-label": ariaLabel,
  size = "md",
}: {
  value: T;
  onChange: (value: T) => void;
  options: SelectOption<T>[];
  className?: string;
  placeholder?: string;
  disabled?: boolean;
  "aria-label"?: string;
  size?: "sm" | "md";
}) {
  const groups: { name: string | undefined; items: SelectOption<T>[] }[] = [];
  for (const option of options) {
    const last = groups[groups.length - 1];
    if (last && last.name === option.group) last.items.push(option);
    else groups.push({ name: option.group, items: [option] });
  }
  return (
    <RSelect.Root value={value} onValueChange={(next) => onChange(next as T)} disabled={disabled}>
      <RSelect.Trigger
        aria-label={ariaLabel}
        className={cn(
          "inline-flex items-center justify-between gap-1.5 rounded-md border border-line-strong bg-app px-2 text-left outline-hidden transition-colors",
          "hover:border-fg-faint focus-visible:border-accent focus-visible:ring-2 focus-visible:ring-accent-soft data-[state=open]:border-accent",
          "disabled:opacity-50",
          size === "sm" ? "h-[24px] text-sm" : "h-[28px] text-base",
          className ?? "w-full",
        )}
      >
        <span className="min-w-0 truncate">
          <RSelect.Value placeholder={placeholder} />
        </span>
        <RSelect.Icon>
          <ChevronDown size={13} className="text-fg-faint" />
        </RSelect.Icon>
      </RSelect.Trigger>
      <RSelect.Portal>
        <RSelect.Content
          position="popper"
          sideOffset={4}
          collisionPadding={8}
          className="z-[60] max-h-(--radix-select-content-available-height) min-w-(--radix-select-trigger-width) animate-pop-in overflow-hidden rounded-lg border border-line-strong bg-raised text-fg shadow-pop"
        >
          <RSelect.Viewport className="p-1">
            {groups.map((group, index) => (
              <RSelect.Group key={group.name ?? index}>
                {group.name && (
                  <RSelect.Label className="px-2 pt-1.5 pb-1 text-xs font-semibold tracking-wide text-fg-faint uppercase">
                    {group.name}
                  </RSelect.Label>
                )}
                {group.items.map((option) => (
                  <RSelect.Item
                    key={option.value}
                    value={option.value}
                    disabled={option.disabled}
                    className="relative flex min-h-[28px] cursor-default items-center gap-2 rounded-md py-1 pr-7 pl-2 outline-hidden select-none data-[disabled]:opacity-40 data-[highlighted]:bg-hover"
                  >
                    {option.icon && <span className="text-fg-muted">{option.icon}</span>}
                    <div className="min-w-0">
                      <RSelect.ItemText>{option.label}</RSelect.ItemText>
                      {option.hint && <div className="text-sm leading-tight text-fg-faint">{option.hint}</div>}
                    </div>
                    <RSelect.ItemIndicator className="absolute right-2 text-accent">
                      <Check size={14} />
                    </RSelect.ItemIndicator>
                  </RSelect.Item>
                ))}
              </RSelect.Group>
            ))}
          </RSelect.Viewport>
        </RSelect.Content>
      </RSelect.Portal>
    </RSelect.Root>
  );
}

// ---------------------------------------------------------------------------------------------
// Switch / segmented / slider
// ---------------------------------------------------------------------------------------------

export function Switch({
  checked,
  onChange,
  disabled,
  tone = "accent",
  "aria-label": ariaLabel,
  id,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
  tone?: "accent" | "midi";
  "aria-label"?: string;
  id?: string;
}) {
  return (
    <RSwitch.Root
      id={id}
      checked={checked}
      onCheckedChange={onChange}
      disabled={disabled}
      aria-label={ariaLabel}
      className={cn(
        "relative inline-flex h-[18px] w-[32px] shrink-0 items-center rounded-full border border-transparent bg-active transition-colors",
        "disabled:opacity-50",
        tone === "midi" ? "data-[state=checked]:bg-midi" : "data-[state=checked]:bg-accent",
      )}
    >
      <RSwitch.Thumb className="block size-[14px] translate-x-[1px] rounded-full bg-white shadow-sm transition-transform data-[state=checked]:translate-x-[15px]" />
    </RSwitch.Root>
  );
}

export function Segmented<T extends string>({
  value,
  onChange,
  options,
  className,
  size = "md",
}: {
  value: T;
  onChange: (value: T) => void;
  options: { value: T; label: ReactNode; title?: string }[];
  className?: string;
  size?: "sm" | "md";
}) {
  return (
    <div
      role="radiogroup"
      className={cn("inline-flex rounded-md border border-line-strong bg-app p-[2px]", className)}
    >
      {options.map((option) => (
        <button
          key={option.value}
          type="button"
          role="radio"
          aria-checked={option.value === value}
          title={option.title}
          onClick={() => onChange(option.value)}
          className={cn(
            "flex-1 rounded-[5px] px-2.5 whitespace-nowrap transition-colors",
            size === "sm" ? "h-[20px] text-sm" : "h-[22px] text-sm",
            option.value === value
              ? "bg-active font-medium text-fg shadow-xs"
              : "text-fg-muted hover:text-fg",
          )}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}

/** A dual-thumb slider for inclusive ranges. */
export function RangeSlider({
  min,
  max,
  value,
  onChange,
  tone = "midi",
  marker,
}: {
  min: number;
  max: number;
  value: [number, number];
  onChange: (value: [number, number]) => void;
  tone?: "midi" | "accent";
  /** A live value to show as a tick on the track (e.g. the control's current position). */
  marker?: number | null;
}) {
  const thumb =
    "block size-[14px] rounded-full border-2 bg-surface shadow-sm outline-hidden transition-transform hover:scale-110 focus-visible:ring-2 focus-visible:ring-accent-soft " +
    (tone === "midi" ? "border-midi" : "border-accent");
  return (
    <Slider.Root
      min={min}
      max={max}
      step={1}
      minStepsBetweenThumbs={0}
      value={value}
      onValueChange={(next) => onChange([next[0], next[1]] as [number, number])}
      className="relative flex h-[20px] w-full touch-none items-center select-none"
    >
      <Slider.Track className="relative h-[4px] grow rounded-full bg-active">
        <Slider.Range className={cn("absolute h-full rounded-full", tone === "midi" ? "bg-midi" : "bg-accent")} />
        {marker !== null && marker !== undefined && (
          <span
            className="absolute top-1/2 h-[12px] w-[2px] -translate-x-1/2 -translate-y-1/2 rounded-full bg-fg"
            style={{ left: `${((clamp(marker, min, max) - min) / (max - min || 1)) * 100}%` }}
          />
        )}
      </Slider.Track>
      <Slider.Thumb className={thumb} aria-label="Minimum" />
      <Slider.Thumb className={thumb} aria-label="Maximum" />
    </Slider.Root>
  );
}
