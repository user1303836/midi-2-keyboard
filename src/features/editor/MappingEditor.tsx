import { ArrowRight, Check, Copy, FolderInput, MoreHorizontal, Play, Radio, Trash2, X } from "lucide-react";
import { useEffect, useState } from "react";

import type { Action } from "@/bindings/Action";
import type { Behavior } from "@/bindings/Behavior";
import type { Mapping } from "@/bindings/Mapping";
import type { Source } from "@/bindings/Source";
import { Button, IconButton } from "@/components/ui/button";
import { Switch } from "@/components/ui/inputs";
import { Badge, Collapsible, Kbd } from "@/components/ui/misc";
import {
  Menu,
  MenuContent,
  MenuItem,
  MenuSeparator,
  MenuSub,
  MenuSubContent,
  MenuSubTrigger,
  MenuTrigger,
} from "@/components/ui/overlays";
import { conditionText, mappingSentence, rateLimitText, sourceQualifiers, sourceTitle } from "@/lib/describe";
import { useLearn, type LearnResult } from "@/lib/learn";
import {
  behaviorTypeInfo,
  defaultBehavior,
  isEmptyAction,
  isFullRange,
  primaryAction,
  rangeLimit,
  sourceShape,
  suggestedBehavior,
  supportsRateLimit,
  type BehaviorType,
} from "@/lib/model";
import { api, useApp, useOctave } from "@/lib/store";
import { cn } from "@/lib/utils";

import { BehaviorEditor } from "./BehaviorEditor";
import { ConditionsEditor, RateLimitEditor } from "./ExtrasEditor";
import { SourceEditor } from "./SourceEditor";
import { SourceIcon } from "../mappings/tokens";

const BUTTON_BEHAVIORS: BehaviorType[] = ["trigger", "hold", "toggle", "tap_hold", "multi_tap", "cycle"];

/**
 * Keeps source and behavior compatible when one of them changes:
 * - a behavior that needs press/release on a full-range continuous control narrows the range to
 *   the upper half (otherwise the control would count as "pressed" forever);
 * - a source whose physical shape changed gets the behavior that suits it, if the current one
 *   does not.
 */
function reconcile(previous: Mapping, next: Mapping): Mapping {
  let result = next;
  const before = sourceShape(previous.source.event);
  const after = sourceShape(result.source.event);

  if (previous.source !== next.source && before !== after) {
    if (!behaviorTypeInfo(result.behavior.type).fits.includes(after)) {
      result = { ...result, behavior: defaultBehavior(suggestedBehavior(after), result.behavior) };
    }
  }

  if (previous.behavior.type !== result.behavior.type || previous.source !== result.source) {
    const event = result.source.event;
    const limit = rangeLimit(event);
    const absolute = event.type !== "control_change" || event.encoding === "absolute";
    if (
      previous.behavior.type !== result.behavior.type &&
      BUTTON_BEHAVIORS.includes(result.behavior.type) &&
      limit !== null &&
      absolute &&
      "range" in event &&
      isFullRange(event)
    ) {
      const range = { min: Math.ceil((limit + 1) / 2), max: limit };
      result = { ...result, source: { ...result.source, event: { ...event, range } as Source["event"] } };
    }
  }

  if (!supportsRateLimit(result.behavior.type) && result.rate_limit) {
    result = { ...result, rate_limit: null };
  }
  return result;
}

export function MappingEditor({ mapping }: { mapping: Mapping }) {
  const updateMapping = useApp((state) => state.updateMapping);
  const removeMapping = useApp((state) => state.removeMapping);
  const cloneMapping = useApp((state) => state.cloneMapping);
  const moveMappingToProfile = useApp((state) => state.moveMappingToProfile);
  const selectMapping = useApp((state) => state.selectMapping);
  const finishWizard = useApp((state) => state.finishWizard);
  const wizard = useApp((state) => state.wizardMappingId === mapping.id);
  const profiles = useApp((state) => state.config.profiles);
  const config = useApp((state) => state.config);
  const toast = useApp((state) => state.toast);
  const undo = useApp((state) => state.undo);
  const octave = useOctave();

  const commitDraft = useApp((state) => state.commitDraft);

  const update = (change: (m: Mapping) => Mapping, key?: string) =>
    updateMapping(mapping.id, (current) => reconcile(current, change(current)), key);

  // Closing a quick-add that has not learned anything yet simply discards the draft.
  const close = () => selectMapping(null);

  const remove = () => {
    removeMapping(mapping.id);
    toast("Mapping deleted", { action: { label: "Undo", run: undo } });
  };

  const applyLearned = (result: LearnResult) => update((m) => ({ ...m, source: result.source }));

  const shape = sourceShape(mapping.source.event);
  const placeholder = `${sourceTitle(mapping.source.event, octave)} mapping`;

  return (
    <aside
      aria-label="Mapping editor"
      className="flex h-full w-[452px] shrink-0 animate-slide-in flex-col border-l bg-surface"
      onKeyDown={(event) => {
        if (event.key === "Escape" && !event.defaultPrevented) close();
      }}
    >
      <header className="flex h-[46px] shrink-0 items-center gap-2 border-b px-3">
        <input
          aria-label="Mapping name"
          value={mapping.name}
          placeholder={placeholder}
          onChange={(event) => update((m) => ({ ...m, name: event.target.value }), "name")}
          className="h-[28px] min-w-0 flex-1 rounded-md border border-transparent bg-transparent px-1.5 text-lg font-semibold outline-hidden placeholder:font-normal placeholder:text-fg-faint hover:border-line-strong focus:border-accent"
        />
        <label className="flex items-center gap-1.5 text-sm text-fg-muted">
          <Switch
            tone="midi"
            aria-label="Mapping enabled"
            checked={mapping.enabled}
            onChange={(enabled) => update((m) => ({ ...m, enabled }))}
          />
        </label>
        <Menu>
          <MenuTrigger asChild>
            <IconButton label="More">
              <MoreHorizontal size={16} />
            </IconButton>
          </MenuTrigger>
          <MenuContent>
            <MenuItem icon={<Copy size={14} />} shortcut="Ctrl+D" onSelect={() => cloneMapping(mapping.id)}>
              Duplicate
            </MenuItem>
            {profiles.length > 1 && (
              <MenuSub>
                <MenuSubTrigger icon={<FolderInput size={14} />}>Move to profile</MenuSubTrigger>
                <MenuSubContent>
                  {profiles
                    .filter((p) => !p.mappings.some((m) => m.id === mapping.id))
                    .map((p) => (
                      <MenuItem key={p.id} onSelect={() => moveMappingToProfile(mapping.id, p.id)}>
                        {p.name}
                      </MenuItem>
                    ))}
                </MenuSubContent>
              </MenuSub>
            )}
            <MenuSeparator />
            <MenuItem tone="danger" icon={<Trash2 size={14} />} shortcut="Del" onSelect={remove}>
              Delete
            </MenuItem>
          </MenuContent>
        </Menu>
        <IconButton label="Close" shortcut="Esc" onClick={close}>
          <X size={16} />
        </IconButton>
      </header>

      {wizard ? (
        <Wizard
          mapping={mapping}
          onSource={(result) => {
            update((m) => ({
              ...m,
              source: result.source,
              behavior: defaultBehavior(suggestedBehavior(result.shape), m.behavior),
            }));
            // It has a real source now: make it a real mapping (it appears in the list).
            commitDraft();
          }}
          onBehavior={(behavior, key) => update((m) => ({ ...m, behavior }), key)}
          onDone={finishWizard}
        />
      ) : (
        <div className="min-h-0 flex-1 overflow-y-auto">
          <Section step="When" tone="midi" title="this comes in">
            <SourceEditor
              source={mapping.source}
              onChange={(source, key) => update((m) => ({ ...m, source }), key && `src:${key}`)}
              onLearned={applyLearned}
            />
          </Section>

          <Section step="Do" tone="key" title="this">
            <BehaviorEditor
              behavior={mapping.behavior}
              shape={shape}
              onChange={(behavior, key) => update((m) => ({ ...m, behavior }), key && `beh:${key}`)}
            />
          </Section>

          <div className="flex flex-col gap-1 px-4 py-3">
            {supportsRateLimit(mapping.behavior.type) && (
              <Collapsible
                title="Rate limit"
                defaultOpen={mapping.rate_limit !== null}
                badge={mapping.rate_limit && <Badge tone="accent">{rateLimitText(mapping.rate_limit)}</Badge>}
                summary={mapping.rate_limit ? undefined : "off — fires as often as the input arrives"}
              >
                <RateLimitEditor
                  value={mapping.rate_limit}
                  onChange={(rate_limit, key) => update((m) => ({ ...m, rate_limit }), key && `rate:${key}`)}
                />
              </Collapsible>
            )}
            <Collapsible
              title="Conditions"
              defaultOpen={mapping.conditions.length > 0}
              badge={mapping.conditions.length > 0 && <Badge tone="accent">{mapping.conditions.length}</Badge>}
              summary={
                mapping.conditions.length
                  ? mapping.conditions.map((c) => conditionText(c, config, octave)).join(", ")
                  : "always active"
              }
            >
              <ConditionsEditor
                value={mapping.conditions}
                onChange={(conditions, key) => update((m) => ({ ...m, conditions }), key && `cond:${key}`)}
              />
            </Collapsible>
            <Collapsible title="Try it" summary="send the action without touching the controller">
              <TestPanel action={primaryAction(mapping.behavior)} />
            </Collapsible>
          </div>

          <div className="selectable border-t px-4 py-3 text-sm leading-relaxed text-fg-faint">
            {mappingSentence(mapping, config, octave)}
          </div>
        </div>
      )}
    </aside>
  );
}

function Section({
  step,
  title,
  tone,
  children,
}: {
  step: string;
  title: string;
  tone: "midi" | "key";
  children: React.ReactNode;
}) {
  return (
    <section className="border-b px-4 pt-3.5 pb-4">
      <h3 className="mb-2.5 flex items-baseline gap-1.5">
        <span className={cn("text-sm font-bold tracking-wider uppercase", tone === "midi" ? "text-midi" : "text-key")}>
          {step}
        </span>
        <span className="text-sm text-fg-faint">{title}</span>
      </h3>
      {children}
    </section>
  );
}

/** Fires the mapping's action after a countdown, so the user can focus the target window. */
function TestPanel({ action }: { action: Action | null }) {
  const [remaining, setRemaining] = useState<number | null>(null);
  const empty = isEmptyAction(action);

  useEffect(() => {
    if (remaining === null) return;
    if (remaining <= 0) {
      const timer = setTimeout(() => setRemaining(null), 600);
      return () => clearTimeout(timer);
    }
    const timer = setTimeout(() => setRemaining(remaining - 1), 1000);
    return () => clearTimeout(timer);
  }, [remaining]);

  const run = async (delaySeconds: number) => {
    if (!action) return;
    const backend = await api();
    await backend.testAction(action, delaySeconds * 1000);
    setRemaining(delaySeconds);
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-2">
        <Button icon={<Play size={13} />} disabled={empty || remaining !== null} onClick={() => run(3)}>
          {remaining === null ? "Send in 3 seconds" : remaining > 0 ? `Sending in ${remaining}…` : "Sent"}
        </Button>
        <span className="text-sm text-fg-faint">Click, then switch to the window that should receive it.</span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------------------------
// Quick add: two gestures and you are done.
// ---------------------------------------------------------------------------------------------

function Wizard({
  mapping,
  onSource,
  onBehavior,
  onDone,
}: {
  mapping: Mapping;
  onSource: (result: LearnResult) => void;
  onBehavior: (behavior: Behavior, coalesceKey?: string) => void;
  onDone: () => void;
}) {
  const [stage, setStage] = useState<"learn" | "action">("learn");
  const [learned, setLearned] = useState<LearnResult | null>(null);
  const multipleDevices = useApp((state) => state.ports.filter((p) => p.state === "connected").length > 1);
  const noDevices = useApp((state) => state.ports.filter((p) => p.state === "connected").length === 0);
  const config = useApp((state) => state.config);
  const octave = useOctave();

  const learn = useLearn(stage === "learn", { pinDevice: multipleDevices }, (result) => {
    setLearned(result);
    onSource(result);
    setStage("action");
  });

  const shape = sourceShape(mapping.source.event);
  const done = !isEmptyAction(primaryAction(mapping.behavior));

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="flex flex-col gap-3 p-4">
        {/* Step 1 */}
        <div
          className={cn(
            "rounded-xl border p-4 transition-colors",
            stage === "learn"
              ? "animate-listen border-midi bg-midi-soft [--ring-color:var(--midi-line)]"
              : "border-line bg-app",
          )}
        >
          <div className="flex items-center gap-2.5">
            <StepDot n={1} done={stage !== "learn"} tone="midi" />
            <div className="min-w-0 flex-1">
              {stage === "learn" ? (
                <>
                  <div className="text-lg font-semibold">
                    {learn.phase === "detecting"
                      ? `${sourceTitle(learn.preview.source.event, octave)} — looks like ${learn.preview.what}`
                      : "Press or move a control on your MIDI device"}
                  </div>
                  <div className="text-fg-muted">
                    {learn.phase === "detecting"
                      ? learn.hint
                      : noDevices
                        ? "No MIDI device is connected yet — plug one in, it will be picked up automatically."
                        : "A key, a pad, a knob, a fader, a pedal… anything."}
                  </div>
                </>
              ) : (
                <div className="flex items-center gap-2">
                  <SourceIcon event={mapping.source.event} className="text-midi" />
                  <span className="text-lg font-semibold">{sourceTitle(mapping.source.event, octave)}</span>
                  <span className="truncate text-fg-muted">
                    {[learned?.what, ...sourceQualifiers(mapping.source, config)].filter(Boolean).join(" · ")}
                  </span>
                </div>
              )}
            </div>
            {stage === "learn" ? (
              <Radio size={20} className="shrink-0 animate-pulse text-midi" />
            ) : (
              <Button size="sm" variant="ghost" onClick={() => setStage("learn")}>
                Redo
              </Button>
            )}
          </div>
        </div>

        <ArrowRight size={16} className="mx-auto rotate-90 text-fg-faint" />

        {/* Step 2 */}
        <div
          className={cn(
            "rounded-xl border p-4 transition-colors",
            stage === "action" ? "border-key-line bg-key-soft" : "border-line bg-app opacity-60",
          )}
        >
          <div className="mb-3 flex items-center gap-2.5">
            <StepDot n={2} done={done} tone="key" />
            <div className="min-w-0 flex-1">
              <div className="text-lg font-semibold">
                {shape === "continuous" || shape === "relative"
                  ? "Now press the key or shortcut it should send"
                  : "Now press the key or shortcut to send"}
              </div>
              {stage === "action" && (
                <div className="text-fg-muted">
                  {shape === "relative"
                    ? "First for turning up, then set the other direction below."
                    : "Just press it on your computer keyboard."}
                </div>
              )}
            </div>
          </div>
          {stage === "action" && (
            <BehaviorEditor
              key={mapping.behavior.type}
              behavior={mapping.behavior}
              shape={shape}
              autoCapture
              onChange={onBehavior}
            />
          )}
        </div>
      </div>

      <div className="mt-auto flex items-center gap-2 border-t px-4 py-3">
        <span className="min-w-0 flex-1 text-sm text-fg-faint">
          {stage === "learn" ? (
            <>
              No controller at hand?{" "}
              <button type="button" className="underline underline-offset-2 hover:text-fg" onClick={onDone}>
                Set it up by hand
              </button>
            </>
          ) : (
            <>
              Rate limits, conditions and more are under <Kbd>All options</Kbd>
            </>
          )}
        </span>
        <Button onClick={onDone}>All options</Button>
        <Button variant="primary" icon={<Check size={14} />} disabled={!done} onClick={() => {
          onDone();
          useApp.getState().selectMapping(null);
        }}>
          Done
        </Button>
      </div>
    </div>
  );
}

function StepDot({ n, done, tone }: { n: number; done: boolean; tone: "midi" | "key" }) {
  return (
    <span
      className={cn(
        "flex size-[26px] shrink-0 items-center justify-center rounded-full text-base font-bold",
        tone === "midi" ? "bg-midi text-app" : "bg-key text-app",
      )}
    >
      {done ? <Check size={15} strokeWidth={3} /> : n}
    </span>
  );
}
