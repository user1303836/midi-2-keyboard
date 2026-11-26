// The visual vocabulary for mappings: how a source, an action and a behavior are drawn wherever
// they appear (list rows, editor, monitor).

import {
  ArrowDown,
  ArrowUp,
  Binary,
  CircleDot,
  ExternalLink,
  Gauge,
  Layers,
  ListMusic,
  ListOrdered,
  Mouse,
  MousePointerClick,
  Move,
  Music2,
  Music4,
  Pause,
  Play,
  RotateCw,
  SlidersHorizontal,
  Square,
  StepForward,
  Terminal,
  Timer,
  Type,
  Waves,
  Zap,
} from "lucide-react";
import type { ReactNode } from "react";

import type { Action } from "@/bindings/Action";
import type { Behavior } from "@/bindings/Behavior";
import type { HoldTarget } from "@/bindings/HoldTarget";
import type { SourceEvent } from "@/bindings/SourceEvent";
import { KeyCombo } from "@/components/ui/misc";
import { actionText, targetText } from "@/lib/describe";
import { isEmptyAction, sourceShape } from "@/lib/model";
import { useApp } from "@/lib/store";
import { cn, formatMs } from "@/lib/utils";

export function SourceIcon({ event, size = 15, className }: { event: SourceEvent; size?: number; className?: string }) {
  const props = { size, className, strokeWidth: 2 };
  switch (event.type) {
    case "note":
      return <Music2 {...props} />;
    case "chord":
      return <Music4 {...props} />;
    case "control_change": {
      const shape = sourceShape(event);
      if (shape === "relative") return <RotateCw {...props} />;
      if (shape === "button") return <CircleDot {...props} />;
      return <SlidersHorizontal {...props} />;
    }
    case "control_change14":
    case "nrpn":
    case "rpn":
      return <SlidersHorizontal {...props} />;
    case "program_change":
    case "song_select":
      return <ListMusic {...props} />;
    case "pitch_bend":
      return <Waves {...props} />;
    case "channel_pressure":
    case "poly_pressure":
    case "song_position":
      return <Gauge {...props} />;
    case "sys_ex":
      return <Binary {...props} />;
    case "clock":
      return <Timer {...props} />;
    case "system":
      if (event.message === "stop") return <Square {...props} />;
      if (event.message === "continue") return <StepForward {...props} />;
      return <Play {...props} />;
  }
}

function Token({ icon, children, muted }: { icon: ReactNode; children: ReactNode; muted?: boolean }) {
  return (
    <span
      className={cn(
        "inline-flex h-[22px] max-w-full items-center gap-1 rounded-[5px] border px-1.5 text-sm whitespace-nowrap",
        muted ? "border-line-strong bg-hover text-fg-faint italic" : "border-key-line bg-key-soft text-key",
      )}
    >
      <span className="shrink-0">{icon}</span>
      <span className="truncate">{children}</span>
    </span>
  );
}

/** An action drawn as keycaps or an icon chip. */
export function ActionView({ action, small = true }: { action: Action | null; small?: boolean }) {
  // Only profile switches need to look anything up; everything else must not re-render on edits.
  const config = useApp((state) => (action?.type === "switch_profile" ? state.config : null));
  if (!action) return <span className="text-fg-faint">—</span>;
  const text = actionText(action, config);
  switch (action.type) {
    case "keys":
      return action.keys.length ? (
        <KeyCombo keys={action.keys} small={small} />
      ) : (
        <Token muted icon={<Zap size={12} />}>
          no keys yet
        </Token>
      );
    case "text":
      return <Token icon={<Type size={12} />}>{text.replace(/^type /, "")}</Token>;
    case "mouse_click":
      return <Token icon={<MousePointerClick size={12} />}>{text}</Token>;
    case "mouse_scroll":
      return <Token icon={<Mouse size={12} />}>{text}</Token>;
    case "mouse_move":
      return <Token icon={<Move size={12} />}>{text}</Token>;
    case "sequence":
      return <Token icon={<ListOrdered size={12} />}>{text}</Token>;
    case "switch_profile":
      return <Token icon={<Layers size={12} />}>{text}</Token>;
    case "layer":
      return <Token icon={<Layers size={12} />}>{text}</Token>;
    case "engine":
      return <Token icon={<Pause size={12} />}>{text}</Token>;
    case "run":
      return <Token icon={<Terminal size={12} />}>{text}</Token>;
    case "open":
      return <Token icon={<ExternalLink size={12} />}>{text}</Token>;
  }
}

export function TargetView({ target, small = true }: { target: HoldTarget; small?: boolean }) {
  switch (target.type) {
    case "keys":
      return target.keys.length ? (
        <KeyCombo keys={target.keys} small={small} />
      ) : (
        <Token muted icon={<Zap size={12} />}>
          no keys yet
        </Token>
      );
    case "mouse_button":
      return <Token icon={<MousePointerClick size={12} />}>{targetText(target)}</Token>;
    case "layer":
      return <Token icon={<Layers size={12} />}>{targetText(target)}</Token>;
  }
}

function Verb({ children }: { children: ReactNode }) {
  return <span className="shrink-0 text-sm text-fg-muted">{children}</span>;
}

/** The right-hand side of a mapping row: what the behavior does, as words + tokens. */
export function BehaviorSummary({ behavior }: { behavior: Behavior }) {
  const b = behavior;
  switch (b.type) {
    case "trigger":
      return (
        <>
          {b.on_press && <ActionView action={b.on_press} />}
          {b.on_release && (
            <>
              <Verb>{b.on_press ? "then on release" : "on release"}</Verb>
              <ActionView action={b.on_release} />
            </>
          )}
          {!b.on_press && !b.on_release && <Verb>does nothing yet</Verb>}
          {b.repeat && <Verb>· repeats every {formatMs(b.repeat.interval_ms)}</Verb>}
        </>
      );
    case "hold":
      return (
        <>
          <Verb>hold</Verb>
          <TargetView target={b.target} />
        </>
      );
    case "toggle":
      return (
        <>
          <Verb>toggle</Verb>
          <TargetView target={b.target} />
        </>
      );
    case "cycle":
      return (
        <>
          <Verb>cycle</Verb>
          {b.actions.slice(0, 3).map((action, index) => (
            <span key={index} className="inline-flex min-w-0 items-center gap-1.5">
              {index > 0 && <span className="text-fg-faint">›</span>}
              <ActionView action={action} />
            </span>
          ))}
          {b.actions.length > 3 && <Verb>+{b.actions.length - 3} more</Verb>}
        </>
      );
    case "tap_hold":
      return (
        <>
          {b.tap && (
            <>
              <Verb>tap</Verb>
              <ActionView action={b.tap} />
            </>
          )}
          {(b.hold || b.hold_target) && <Verb>hold</Verb>}
          {b.hold && <ActionView action={b.hold} />}
          {b.hold_target && <TargetView target={b.hold_target} />}
        </>
      );
    case "multi_tap":
      return (
        <>
          {(
            [
              ["1×", b.single],
              ["2×", b.double],
              ["3×", b.triple],
              ["long", b.hold],
            ] as const
          )
            .filter(([, action]) => action)
            .map(([label, action]) => (
              <span key={label} className="inline-flex min-w-0 items-center gap-1.5">
                <Verb>{label}</Verb>
                <ActionView action={action} />
              </span>
            ))}
        </>
      );
    case "change":
      return <ActionView action={b.action} />;
    case "step":
      return (
        <>
          {b.up && (
            <span className="inline-flex min-w-0 items-center gap-1">
              <ArrowUp size={13} className="shrink-0 text-fg-muted" />
              <ActionView action={b.up} />
            </span>
          )}
          {b.down && (
            <span className="inline-flex min-w-0 items-center gap-1">
              <ArrowDown size={13} className="shrink-0 text-fg-muted" />
              <ActionView action={b.down} />
            </span>
          )}
          {!b.up && !b.down && <Verb>does nothing yet</Verb>}
          {((b.fast?.up && !isEmptyAction(b.fast.up)) || (b.fast?.down && !isEmptyAction(b.fast.down))) && <Verb>+ fast</Verb>}
        </>
      );
  }
}
