import {
  AlertTriangle,
  ArrowDown,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  CheckCircle2,
  ChevronRight,
  Info,
  X,
  XCircle,
} from "lucide-react";
import { useState, type ReactNode } from "react";

import type { Key } from "@/bindings/Key";
import { keyInfo } from "@/lib/keys";
import { platform } from "@/lib/platform";
import { useApp } from "@/lib/store";
import { cn } from "@/lib/utils";

import { Tooltip } from "./overlays";

// ---------------------------------------------------------------------------------------------
// Layout helpers for forms
// ---------------------------------------------------------------------------------------------

/** A labelled row: label on the left, control on the right, optional hint underneath. */
export function Field({
  label,
  hint,
  children,
  align: alignProp,
  className,
}: {
  label: ReactNode;
  hint?: ReactNode;
  children: ReactNode;
  align?: "center" | "start";
  className?: string;
}) {
  // With a hint underneath, the label belongs next to the control, not the middle of the block.
  const align = alignProp ?? (hint ? "start" : "center");
  return (
    <div className={cn("flex gap-3", align === "center" ? "items-center" : "items-start", className)}>
      <div className={cn("w-[104px] shrink-0 text-fg-muted", align === "start" && "pt-[5px]")}>{label}</div>
      <div className="min-w-0 flex-1">
        {children}
        {hint && <div className="mt-1 text-sm leading-snug text-fg-faint">{hint}</div>}
      </div>
    </div>
  );
}

/** A switch with a title and explanation, used for boolean options. */
export function Option({
  title,
  description,
  children,
  className,
}: {
  title: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  className?: string;
}) {
  return (
    <div className={cn("flex items-start gap-4", className)}>
      <div className="min-w-0 flex-1">
        <div className="text-fg">{title}</div>
        {description && <div className="mt-0.5 text-sm leading-snug text-fg-faint">{description}</div>}
      </div>
      <div className="flex shrink-0 items-center pt-[1px]">{children}</div>
    </div>
  );
}

/** A collapsible group. `summary` is shown next to the title while collapsed. */
export function Collapsible({
  title,
  summary,
  defaultOpen = false,
  open: controlled,
  onOpenChange,
  children,
  badge,
}: {
  title: ReactNode;
  summary?: ReactNode;
  defaultOpen?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  children: ReactNode;
  badge?: ReactNode;
}) {
  const [internal, setInternal] = useState(defaultOpen);
  const open = controlled ?? internal;
  const toggle = () => {
    setInternal(!open);
    onOpenChange?.(!open);
  };
  return (
    <div>
      <button
        type="button"
        onClick={toggle}
        aria-expanded={open}
        className="group flex w-full items-center gap-1.5 rounded-md py-1 text-left text-fg-muted hover:text-fg"
      >
        <ChevronRight size={14} className={cn("shrink-0 transition-transform", open && "rotate-90")} />
        <span className="font-medium">{title}</span>
        {badge}
        {!open && summary && <span className="min-w-0 truncate pl-1 text-sm text-fg-faint">{summary}</span>}
      </button>
      {open && <div className="animate-fade-in pt-2 pb-1 pl-[21px]">{children}</div>}
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Small display atoms
// ---------------------------------------------------------------------------------------------

type Tone = "neutral" | "midi" | "key" | "accent" | "warn" | "danger";

const TONES: Record<Tone, string> = {
  neutral: "bg-hover text-fg-muted border-line-strong",
  midi: "bg-midi-soft text-midi border-midi-line",
  key: "bg-key-soft text-key border-key-line",
  accent: "bg-accent-soft text-accent border-accent-line",
  warn: "bg-warn-soft text-warn border-transparent",
  danger: "bg-danger-soft text-danger border-transparent",
};

export function Badge({
  tone = "neutral",
  children,
  className,
  title,
}: {
  tone?: Tone;
  children: ReactNode;
  className?: string;
  title?: string;
}) {
  return (
    <span
      title={title}
      className={cn(
        "inline-flex h-[20px] shrink-0 items-center gap-1 rounded-[5px] border px-1.5 text-sm leading-none whitespace-nowrap",
        TONES[tone],
        className,
      )}
    >
      {children}
    </span>
  );
}

const ARROWS: Partial<Record<Key, typeof ArrowUp>> = {
  ArrowUp,
  ArrowDown,
  ArrowLeft,
  ArrowRight,
};

/** One keycap. */
export function Keycap({ k, small }: { k: Key; small?: boolean }) {
  const info = keyInfo(k);
  const Arrow = ARROWS[k];
  return (
    <Tooltip content={info.name !== info.label ? info.name : null} side="top">
      <kbd className={cn("keycap font-sans", small && "keycap-sm")}>
        {Arrow ? <Arrow size={small ? 12 : 14} strokeWidth={2.6} aria-label={info.name} /> : info.label}
      </kbd>
    </Tooltip>
  );
}

/** A key combination as a row of keycaps. */
export function KeyCombo({ keys, small, className }: { keys: readonly Key[]; small?: boolean; className?: string }) {
  if (keys.length === 0) return <span className="text-fg-faint italic">no keys</span>;
  const mac = platform() === "macos";
  return (
    <span className={cn("inline-flex items-center", mac ? "gap-[3px]" : "gap-[2px]", className)}>
      {keys.map((key, index) => (
        <span key={`${key}-${index}`} className="inline-flex items-center gap-[2px]">
          {index > 0 && !mac && <span className="text-sm text-fg-faint">+</span>}
          <Keycap k={key} small={small} />
        </span>
      ))}
    </span>
  );
}

/** A plain-text keycap for arbitrary labels (hints such as "Esc" or "Enter"). */
export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="keycap keycap-sm font-sans">{children}</kbd>;
}

// ---------------------------------------------------------------------------------------------
// Toasts
// ---------------------------------------------------------------------------------------------

const TOAST_ICON = {
  info: <Info size={15} className="text-accent" />,
  success: <CheckCircle2 size={15} className="text-midi" />,
  warning: <AlertTriangle size={15} className="text-warn" />,
  error: <XCircle size={15} className="text-danger" />,
};

export function Toasts() {
  const toasts = useApp((state) => state.toasts);
  const dismiss = useApp((state) => state.dismissToast);
  return (
    <div className="pointer-events-none fixed bottom-12 left-1/2 z-[90] flex -translate-x-1/2 flex-col items-center gap-2">
      {toasts.map((toast) => (
        <div
          key={toast.id}
          role="status"
          className="pointer-events-auto flex max-w-[520px] animate-slide-up items-start gap-2.5 rounded-lg border border-line-strong bg-raised py-2 pr-2 pl-3 shadow-pop"
        >
          <span className="pt-[2px]">{TOAST_ICON[toast.kind]}</span>
          <span className="selectable min-w-0 flex-1 leading-snug">{toast.message}</span>
          {toast.action && (
            <button
              type="button"
              className="h-[22px] shrink-0 rounded-md px-2 font-semibold text-accent hover:bg-accent-soft"
              onClick={() => {
                toast.action!.run();
                dismiss(toast.id);
              }}
            >
              {toast.action.label}
            </button>
          )}
          <button
            type="button"
            aria-label="Dismiss"
            className="inline-flex size-[22px] shrink-0 items-center justify-center rounded-md text-fg-faint hover:bg-hover hover:text-fg"
            onClick={() => dismiss(toast.id)}
          >
            <X size={13} />
          </button>
        </div>
      ))}
    </div>
  );
}
