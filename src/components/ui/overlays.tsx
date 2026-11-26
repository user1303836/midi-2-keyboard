import {
  AlertDialog,
  ContextMenu as RContextMenu,
  Dialog as RDialog,
  DropdownMenu,
  Popover as RPopover,
  Tooltip as RTooltip,
} from "radix-ui";
import { Check, X } from "lucide-react";
import type { ComponentProps, ReactNode } from "react";

import { cn } from "@/lib/utils";

// ---------------------------------------------------------------------------------------------
// Tooltip
// ---------------------------------------------------------------------------------------------

export const TooltipProvider = RTooltip.Provider;

export function Tooltip({
  content,
  shortcut,
  side = "bottom",
  children,
}: {
  content: ReactNode;
  shortcut?: string;
  side?: "top" | "bottom" | "left" | "right";
  children: ReactNode;
}) {
  if (!content) return <>{children}</>;
  return (
    <RTooltip.Root>
      <RTooltip.Trigger asChild>{children}</RTooltip.Trigger>
      <RTooltip.Portal>
        <RTooltip.Content
          side={side}
          sideOffset={6}
          collisionPadding={8}
          className="z-[100] max-w-[280px] animate-fade-in rounded-md border border-line-strong bg-raised px-2 py-1 text-sm text-fg shadow-pop"
        >
          {content}
          {shortcut && <span className="ml-2 text-fg-faint">{shortcut}</span>}
        </RTooltip.Content>
      </RTooltip.Portal>
    </RTooltip.Root>
  );
}

// ---------------------------------------------------------------------------------------------
// Popover
// ---------------------------------------------------------------------------------------------

export const Popover = RPopover.Root;
export const PopoverTrigger = RPopover.Trigger;
export const PopoverAnchor = RPopover.Anchor;
export const PopoverClose = RPopover.Close;

export function PopoverContent({
  className,
  align = "start",
  sideOffset = 6,
  ...props
}: ComponentProps<typeof RPopover.Content>) {
  return (
    <RPopover.Portal>
      <RPopover.Content
        align={align}
        sideOffset={sideOffset}
        collisionPadding={10}
        className={cn(
          "z-50 animate-pop-in rounded-lg border border-line-strong bg-raised p-3 text-fg shadow-pop",
          "origin-(--radix-popover-content-transform-origin)",
          className,
        )}
        {...props}
      />
    </RPopover.Portal>
  );
}

// ---------------------------------------------------------------------------------------------
// Dialog
// ---------------------------------------------------------------------------------------------

export function Dialog({
  open,
  onOpenChange,
  title,
  description,
  children,
  width = 520,
  footer,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: ReactNode;
  description?: ReactNode;
  children: ReactNode;
  width?: number;
  footer?: ReactNode;
}) {
  return (
    <RDialog.Root open={open} onOpenChange={onOpenChange}>
      <RDialog.Portal>
        <RDialog.Overlay className="fixed inset-0 z-40 animate-fade-in bg-overlay backdrop-blur-[2px]" />
        <RDialog.Content
          style={{ width }}
          aria-describedby={description ? undefined : undefined}
          className="fixed top-1/2 left-1/2 z-50 flex max-h-[86vh] max-w-[94vw] -translate-x-1/2 -translate-y-1/2 animate-pop-in flex-col rounded-xl border border-line-strong bg-surface shadow-pop"
        >
          <div className="flex items-start gap-3 border-b px-5 py-4">
            <div className="min-w-0 flex-1">
              <RDialog.Title className="text-lg font-semibold">{title}</RDialog.Title>
              {description ? (
                <RDialog.Description className="mt-0.5 text-sm text-fg-muted">{description}</RDialog.Description>
              ) : (
                <RDialog.Description className="sr-only">{title}</RDialog.Description>
              )}
            </div>
            <RDialog.Close
              aria-label="Close"
              className="-mt-1 -mr-2 inline-flex size-[28px] items-center justify-center rounded-md text-fg-muted hover:bg-hover hover:text-fg"
            >
              <X size={16} />
            </RDialog.Close>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">{children}</div>
          {footer && <div className="flex items-center justify-end gap-2 border-t px-5 py-3">{footer}</div>}
        </RDialog.Content>
      </RDialog.Portal>
    </RDialog.Root>
  );
}

export function ConfirmDialog({
  open,
  onOpenChange,
  title,
  description,
  confirmLabel,
  onConfirm,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: ReactNode;
  confirmLabel: string;
  onConfirm: () => void;
}) {
  return (
    <AlertDialog.Root open={open} onOpenChange={onOpenChange}>
      <AlertDialog.Portal>
        <AlertDialog.Overlay className="fixed inset-0 z-40 animate-fade-in bg-overlay backdrop-blur-[2px]" />
        <AlertDialog.Content className="fixed top-1/2 left-1/2 z-50 w-[400px] max-w-[92vw] -translate-x-1/2 -translate-y-1/2 animate-pop-in rounded-xl border border-line-strong bg-surface p-5 shadow-pop">
          <AlertDialog.Title className="text-lg font-semibold">{title}</AlertDialog.Title>
          <AlertDialog.Description className="mt-1.5 text-fg-muted">{description}</AlertDialog.Description>
          <div className="mt-5 flex justify-end gap-2">
            <AlertDialog.Cancel className="h-[30px] rounded-md border border-line-strong bg-raised px-3 hover:bg-hover">
              Cancel
            </AlertDialog.Cancel>
            <AlertDialog.Action
              onClick={onConfirm}
              className="h-[30px] rounded-md bg-danger px-3 font-semibold text-white hover:brightness-110"
            >
              {confirmLabel}
            </AlertDialog.Action>
          </div>
        </AlertDialog.Content>
      </AlertDialog.Portal>
    </AlertDialog.Root>
  );
}

// ---------------------------------------------------------------------------------------------
// Menu
// ---------------------------------------------------------------------------------------------

export const Menu = DropdownMenu.Root;
export const MenuTrigger = DropdownMenu.Trigger;

export function MenuContent({ className, align = "end", ...props }: ComponentProps<typeof DropdownMenu.Content>) {
  return (
    <DropdownMenu.Portal>
      <DropdownMenu.Content
        align={align}
        sideOffset={5}
        collisionPadding={8}
        className={cn(
          "z-50 min-w-[190px] animate-pop-in rounded-lg border border-line-strong bg-raised p-1 text-fg shadow-pop",
          "origin-(--radix-dropdown-menu-content-transform-origin)",
          className,
        )}
        {...props}
      />
    </DropdownMenu.Portal>
  );
}

const ITEM =
  "flex h-[28px] cursor-default items-center gap-2 rounded-md px-2 outline-hidden select-none data-[disabled]:opacity-40 data-[highlighted]:bg-hover";

export function MenuItem({
  icon,
  shortcut,
  tone,
  children,
  className,
  ...props
}: ComponentProps<typeof DropdownMenu.Item> & { icon?: ReactNode; shortcut?: string; tone?: "danger" }) {
  return (
    <DropdownMenu.Item
      className={cn(ITEM, tone === "danger" && "text-danger data-[highlighted]:bg-danger-soft", className)}
      {...props}
    >
      <span className="flex w-4 justify-center text-fg-muted [[data-highlighted]_&]:text-current">{icon}</span>
      <span className="flex-1">{children}</span>
      {shortcut && <span className="pl-4 text-sm text-fg-faint">{shortcut}</span>}
    </DropdownMenu.Item>
  );
}

export function MenuCheckItem({
  checked,
  children,
  ...props
}: ComponentProps<typeof DropdownMenu.CheckboxItem>) {
  return (
    <DropdownMenu.CheckboxItem checked={checked} className={ITEM} {...props}>
      <span className="flex w-4 justify-center text-accent">{checked ? <Check size={14} /> : null}</span>
      <span className="flex-1">{children}</span>
    </DropdownMenu.CheckboxItem>
  );
}

export function MenuSeparator() {
  return <DropdownMenu.Separator className="mx-1 my-1 h-px bg-line" />;
}

export function MenuLabel({ children }: { children: ReactNode }) {
  return (
    <DropdownMenu.Label className="px-2 pt-1.5 pb-1 text-xs font-semibold tracking-wide text-fg-faint uppercase">
      {children}
    </DropdownMenu.Label>
  );
}

// ---------------------------------------------------------------------------------------------
// Context menu (right click)
// ---------------------------------------------------------------------------------------------

export const ContextMenu = RContextMenu.Root;
export const ContextMenuTrigger = RContextMenu.Trigger;

export function ContextMenuContent({ className, ...props }: ComponentProps<typeof RContextMenu.Content>) {
  return (
    <RContextMenu.Portal>
      <RContextMenu.Content
        collisionPadding={8}
        className={cn(
          "z-50 min-w-[200px] animate-pop-in rounded-lg border border-line-strong bg-raised p-1 text-fg shadow-pop",
          className,
        )}
        {...props}
      />
    </RContextMenu.Portal>
  );
}

export function ContextMenuItem({
  icon,
  shortcut,
  tone,
  children,
  className,
  ...props
}: ComponentProps<typeof RContextMenu.Item> & { icon?: ReactNode; shortcut?: string; tone?: "danger" }) {
  return (
    <RContextMenu.Item
      className={cn(ITEM, tone === "danger" && "text-danger data-[highlighted]:bg-danger-soft", className)}
      {...props}
    >
      <span className="flex w-4 justify-center text-fg-muted">{icon}</span>
      <span className="flex-1">{children}</span>
      {shortcut && <span className="pl-4 text-sm text-fg-faint">{shortcut}</span>}
    </RContextMenu.Item>
  );
}

export function ContextMenuSeparator() {
  return <RContextMenu.Separator className="mx-1 my-1 h-px bg-line" />;
}

export const ContextMenuSub = RContextMenu.Sub;

export function ContextMenuSubTrigger({ icon, children }: { icon?: ReactNode; children: ReactNode }) {
  return (
    <RContextMenu.SubTrigger className={cn(ITEM, "data-[state=open]:bg-hover")}>
      <span className="flex w-4 justify-center text-fg-muted">{icon}</span>
      <span className="flex-1">{children}</span>
      <span className="text-fg-faint">›</span>
    </RContextMenu.SubTrigger>
  );
}

export function ContextMenuSubContent({ children }: { children: ReactNode }) {
  return (
    <RContextMenu.Portal>
      <RContextMenu.SubContent
        sideOffset={4}
        collisionPadding={8}
        className="z-50 max-h-[320px] min-w-[170px] animate-pop-in overflow-y-auto rounded-lg border border-line-strong bg-raised p-1 text-fg shadow-pop"
      >
        {children}
      </RContextMenu.SubContent>
    </RContextMenu.Portal>
  );
}

export const MenuSub = DropdownMenu.Sub;

export function MenuSubTrigger({ icon, children }: { icon?: ReactNode; children: ReactNode }) {
  return (
    <DropdownMenu.SubTrigger className={cn(ITEM, "data-[state=open]:bg-hover")}>
      <span className="flex w-4 justify-center text-fg-muted">{icon}</span>
      <span className="flex-1">{children}</span>
      <span className="text-fg-faint">›</span>
    </DropdownMenu.SubTrigger>
  );
}

export function MenuSubContent({ children }: { children: ReactNode }) {
  return (
    <DropdownMenu.Portal>
      <DropdownMenu.SubContent
        sideOffset={4}
        collisionPadding={8}
        className="z-50 min-w-[170px] animate-pop-in rounded-lg border border-line-strong bg-raised p-1 text-fg shadow-pop"
      >
        {children}
      </DropdownMenu.SubContent>
    </DropdownMenu.Portal>
  );
}
