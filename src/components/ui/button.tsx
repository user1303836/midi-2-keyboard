import type { ButtonHTMLAttributes, ReactNode, Ref } from "react";

import { cn } from "@/lib/utils";

import { Tooltip } from "./overlays";

type Variant = "primary" | "secondary" | "ghost" | "danger" | "midi" | "key";
type Size = "sm" | "md" | "lg";

const VARIANTS: Record<Variant, string> = {
  primary: "bg-accent text-accent-fg hover:bg-accent-hover font-semibold shadow-xs",
  secondary: "bg-raised text-fg border border-line-strong hover:bg-hover hover:border-fg-faint",
  ghost: "text-fg-muted hover:text-fg hover:bg-hover",
  danger: "text-danger hover:bg-danger-soft",
  midi: "bg-midi-soft text-midi border border-midi-line hover:bg-midi/20 font-medium",
  key: "bg-key-soft text-key border border-key-line hover:bg-key/20 font-medium",
};

const SIZES: Record<Size, string> = {
  sm: "h-[24px] px-2 text-sm gap-1 rounded-md",
  md: "h-[28px] px-2.5 text-base gap-1.5 rounded-md",
  lg: "h-[34px] px-3.5 text-base gap-2 rounded-lg",
};

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: Variant;
  size?: Size;
  icon?: ReactNode;
  ref?: Ref<HTMLButtonElement>;
}

export function Button({
  variant = "secondary",
  size = "md",
  icon,
  className,
  children,
  type = "button",
  ...props
}: ButtonProps) {
  return (
    <button
      type={type}
      className={cn(
        "inline-flex shrink-0 items-center justify-center whitespace-nowrap transition-colors",
        "disabled:pointer-events-none disabled:opacity-45",
        VARIANTS[variant],
        SIZES[size],
        className,
      )}
      {...props}
    >
      {icon}
      {children}
    </button>
  );
}

export interface IconButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** Accessible name; also shown as the tooltip. */
  label: string;
  size?: "sm" | "md";
  active?: boolean;
  tone?: "default" | "danger";
  shortcut?: string;
  ref?: Ref<HTMLButtonElement>;
}

export function IconButton({
  label,
  size = "md",
  active,
  tone = "default",
  shortcut,
  className,
  children,
  type = "button",
  ...props
}: IconButtonProps) {
  return (
    <Tooltip content={label} shortcut={shortcut}>
      <button
        type={type}
        aria-label={label}
        className={cn(
          "inline-flex shrink-0 items-center justify-center rounded-md transition-colors",
          "disabled:pointer-events-none disabled:opacity-40",
          size === "sm" ? "size-[24px]" : "size-[28px]",
          tone === "danger"
            ? "text-fg-muted hover:bg-danger-soft hover:text-danger"
            : active
              ? "bg-accent-soft text-accent"
              : "text-fg-muted hover:bg-hover hover:text-fg",
          className,
        )}
        {...props}
      >
        {children}
      </button>
    </Tooltip>
  );
}
