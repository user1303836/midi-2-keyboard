import { ArrowRight, Check, Copy, FolderInput, Layers, MoreHorizontal, Play, Radio, Timer, Trash2, X } from "lucide-react";
import { useEffect, useMemo, useState } from "react";

import type { Action } from "@/bindings/Action";
import type { Behavior } from "@/bindings/Behavior";
import type { Mapping } from "@/bindings/Mapping";
import type { RateLimit } from "@/bindings/RateLimit";
import type { Source } from "@/bindings/Source";
import { Button, IconButton } from "@/components/ui/button";
import { Segmented, Switch, TextInput } from "@/components/ui/inputs";
import { Badge, Collapsible } from "@/components/ui/misc";
import {
  Dialog,
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
  ccWithShape,
  defaultBehavior,
  isEmptyAction,
  isFullRange,
  layerSwitches,
  newSource,
  primaryAction,
  rangeLimit,
  sourceShape,
  suggestedBehavior,
  supportsRateLimit,
  UPPER7,
  type BehaviorType,
} from "@/lib/model";
import { findConflicts } from "@/lib/conflicts";
import { api, profileOfMapping, useApp, useKnownLayers, useOctave } from "@/lib/store";
import { cn } from "@/lib/utils";

import { BehaviorEditor } from "./BehaviorEditor";
import { ConditionsEditor, LearnOnce, RateLimitEditor } from "./ExtrasEditor";
import { FeedbackEditor } from "./FeedbackEditor";
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
      // Pitch bend rests in the middle, so its "pressed" part starts well above the rest position.
      const range = { min: event.type === "pitch_bend" ? 12288 : Math.ceil((limit + 1) / 2), max: limit };
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
  const addLayerVariant = useApp((state) => state.addLayerVariant);
  const [variantOpen, setVariantOpen] = useState(false);

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
  const isDraft = useApp((state) => state.draft?.id === mapping.id);
  const placeholder = isDraft ? "New mapping" : `${sourceTitle(mapping.source.event, octave)} mapping`;

  return (
    <aside
      aria-label="Mapping editor"
      className="z-20 flex h-full w-[452px] max-w-full shrink-0 animate-slide-in flex-col border-l bg-surface max-[1180px]:absolute max-[1180px]:top-0 max-[1180px]:right-0 max-[1180px]:bottom-0 max-[1180px]:shadow-pop"
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
            <MenuItem icon={<Layers size={14} />} onSelect={() => setVariantOpen(true)}>
              Add a layer variant…
            </MenuItem>
            {profiles.length > 1 && (
              <MenuSub>
                <MenuSubTrigger icon={<FolderInput size={14} />}>Move to profile</MenuSubTrigger>
                <MenuSubContent>
                  {profiles
                    .filter((p) => !p.mappings.some((m) => m.id === mapping.id))
                    .map((p) => (
                      <MenuItem
                        key={p.id}
                        onSelect={() => {
                          moveMappingToProfile(mapping.id, p.id);
                          toast(`Moved to “${p.name}”`, { action: { label: "Undo", run: undo } });
                        }}
                      >
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
      <LayerVariantDialog
        open={variantOpen}
        onOpenChange={setVariantOpen}
        onCreate={(layer, switchSource, hasSwitch) => {
          addLayerVariant(mapping.id, layer, switchSource);
          toast(
            switchSource
              ? `This control now has a second job while layer “${layer}” is on — hold ${sourceTitle(switchSource.event, octave)} to use it.`
              : hasSwitch
                ? `This control now has a second job while layer “${layer}” is on.`
                : `This control now has a second job while layer “${layer}” is on. Give a pad the behavior Hold → Layer “${layer}” to switch.`,
            { kind: "success" },
          );
        }}
      />

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
          onUpdate={(change) => update(change)}
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
            <RateTip mapping={mapping} onApply={(rate_limit) => update((m) => ({ ...m, rate_limit }))} />
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
            <Collapsible
              title="Light on the controller"
              defaultOpen={mapping.feedback !== null}
              badge={mapping.feedback && <Badge tone="midi">on</Badge>}
              summary={mapping.feedback ? undefined : "off — nothing is sent back"}
            >
              <FeedbackEditor
                mapping={mapping}
                onChange={(feedback, key) => update((m) => ({ ...m, feedback }), key && `fb:${key}`)}
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

/** Asks which layer a mapping should get a variant for. */
function LayerVariantDialog({
  open,
  onOpenChange,
  onCreate,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreate: (layer: string, switchSource: Source | null, hasSwitch: boolean) => void;
}) {
  const layers = useKnownLayers();
  const config = useApp((state) => state.config);
  const pinDevice = useApp((state) => state.ports.filter((p) => p.state === "connected").length > 1);
  const octave = useOctave();
  const [layer, setLayer] = useState("");
  // The control that will turn the layer on, when nothing does yet.
  const [switchSource, setSwitchSource] = useState<Source | null>(null);
  useEffect(() => {
    if (open) {
      setLayer(layers[0] ?? "shift");
      setSwitchSource(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);
  const name = layer.trim();
  const switches = useMemo(() => (name ? layerSwitches(config, name) : []), [config, name]);
  return (
    <Dialog
      open={open}
      onOpenChange={onOpenChange}
      title="Add a layer variant"
      description="Layers work like a Shift key for your controller: while a layer is on, controls can do something else."
      width={460}
      footer={
        <>
          <Button onClick={() => onOpenChange(false)}>Cancel</Button>
          <Button
            variant="primary"
            disabled={!name}
            onClick={() => {
              onCreate(name, switches.length > 0 ? null : switchSource, switches.length > 0);
              onOpenChange(false);
            }}
          >
            Add variant
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <label className="flex items-center gap-3">
          <span className="text-fg-muted">Layer</span>
          <TextInput
            aria-label="Layer name"
            className="w-[200px]"
            list="m2k-layer-variant-names"
            autoFocus
            value={layer}
            onChange={setLayer}
          />
          <datalist id="m2k-layer-variant-names">
            {layers.map((known) => (
              <option key={known} value={known} />
            ))}
          </datalist>
        </label>
        <div className="flex min-h-[28px] flex-wrap items-center gap-2">
          <span className="text-fg-muted">Turned on by</span>
          {switches.length > 0 ? (
            <span>
              <span className="font-semibold text-midi">
                {switches
                  .slice(0, 3)
                  .map((m) => sourceTitle(m.source.event, octave))
                  .join(", ")}
              </span>
              <span className="text-fg-faint"> — already set up</span>
            </span>
          ) : switchSource ? (
            <>
              <span className="flex h-[26px] items-center rounded-md border border-midi-line bg-midi-soft px-2 font-semibold text-midi">
                {sourceTitle(switchSource.event, octave)}
              </span>
              <span className="text-fg-muted">while it is held</span>
              <IconButton label="Choose another control" size="sm" onClick={() => setSwitchSource(null)}>
                <X size={13} />
              </IconButton>
            </>
          ) : (
            <>
              <LearnOnce
                label="Press the pad or button…"
                accept={(log) => {
                  const message = log.message;
                  const device = pinDevice ? log.device : null;
                  if (message.type === "note_on" && message.velocity > 0) {
                    const event = { type: "note" as const, notes: { min: message.note, max: message.note }, velocity: { min: 1, max: 127 } };
                    setSwitchSource({ ...newSource(event), channel: message.channel, device });
                    return true;
                  }
                  if (message.type === "control_change" && message.value >= 64) {
                    const event = { type: "control_change" as const, controller: message.controller, range: { ...UPPER7 }, encoding: "absolute" as const };
                    setSwitchSource({ ...newSource(event), channel: message.channel, device });
                    return true;
                  }
                  return false;
                }}
              />
              <span className="text-sm text-fg-faint">nothing yet — optional</span>
            </>
          )}
        </div>
        <div className="text-sm leading-relaxed text-fg-faint">
          This mapping will only fire while the layer is <b>off</b>, and a copy that only fires while
          it is <b>on</b> is added below it for you to change.
          {switches.length === 0 &&
            (switchSource
              ? " The control above is added as the layer’s switch: the layer is on while you hold it."
              : " Click Learn and press the pad or button that should turn the layer on — or do it later with the behavior Hold and the target Layer.")}
        </div>
      </div>
    </Dialog>
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

/**
 * The owner's own example in one click: a knob bound to a shortcut with "on change" fires for
 * every one of the dozens of messages a turn produces. Say so, and offer the cure.
 */
function RateTip({
  mapping,
  onApply,
  confirm = false,
}: {
  mapping: Mapping;
  onApply: (limit: RateLimit | null) => void;
  /** Keep a line in its place once a limit is set (where no rate limit editor is in sight). */
  confirm?: boolean;
}) {
  const shape = sourceShape(mapping.source.event);
  if (mapping.behavior.type !== "change" || shape === "button" || shape === "pulse") return null;
  if (isEmptyAction(mapping.behavior.action)) return null;
  if (mapping.rate_limit) {
    if (!confirm) return null;
    return (
      <div className="mt-3 flex items-center gap-1.5 text-sm text-fg-muted">
        <Timer size={14} className="shrink-0 text-accent" />
        <span>Limited: {rateLimitText(mapping.rate_limit)}</span>
        <span className="text-fg-faint">·</span>
        <button type="button" className="underline underline-offset-2 hover:text-fg" onClick={() => onApply(null)}>
          remove
        </button>
      </div>
    );
  }
  return (
    <div className="mt-3 flex items-center gap-2 rounded-lg border border-accent-line bg-accent-soft px-2.5 py-2">
      <Timer size={15} className="shrink-0 text-accent" />
      <span className="min-w-0 flex-1 text-sm leading-snug text-fg-muted">
        A {shape === "relative" ? "turn of an encoder" : "turn of a knob"} sends dozens of messages, and this fires
        for every one of them.
      </span>
      <Button size="sm" onClick={() => onApply({ mode: "throttle", interval_ms: 500, trailing: false })}>
        Limit to once per 500 ms
      </Button>
    </div>
  );
}

function Wizard({
  mapping,
  onSource,
  onBehavior,
  onUpdate,
  onDone,
}: {
  mapping: Mapping;
  onSource: (result: LearnResult) => void;
  onBehavior: (behavior: Behavior, coalesceKey?: string) => void;
  onUpdate: (change: (mapping: Mapping) => Mapping) => void;
  onDone: () => void;
}) {
  const [stage, setStage] = useState<"learn" | "action">("learn");
  const [learned, setLearned] = useState<LearnResult | null>(null);
  // Which action slot is listening for keys: the first one, then the next empty one.
  const [armedSlot, setArmedSlot] = useState<number | null>(0);
  const multipleDevices = useApp((state) => state.ports.filter((p) => p.state === "connected").length > 1);
  const noDevices = useApp((state) => state.ports.filter((p) => p.state === "connected").length === 0);
  const config = useApp((state) => state.config);
  const octave = useOctave();

  const learn = useLearn(stage === "learn", { pinDevice: multipleDevices }, (result) => {
    setLearned(result);
    onSource(result);
    setArmedSlot(0);
    setStage("action");
  });

  const event = mapping.source.event;
  const shape = sourceShape(event);
  const done = !isEmptyAction(primaryAction(mapping.behavior));
  // What kind of action is being set up (the heading of step 2 follows it).
  const primaryKind = primaryAction(mapping.behavior)?.type ?? null;
  // Does another mapping of this profile already react to the control that was just learned?
  const clash = useMemo(() => {
    const owner = profileOfMapping(config, mapping.id);
    const found = owner ? findConflicts(config, owner).get(mapping.id) : undefined;
    return found?.find((c) => c.profile === null) ?? null;
  }, [config, mapping.id]);

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
                  <SourceIcon event={mapping.source.event} className="shrink-0 text-midi" />
                  <span className="shrink-0 text-lg font-semibold whitespace-nowrap">{sourceTitle(mapping.source.event, octave)}</span>
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
          {stage === "action" && event.type === "control_change" && (
            <div className="mt-3 flex items-center gap-2 text-sm text-fg-muted">
              <span className="shrink-0">It is a</span>
              <Segmented
                size="sm"
                className="flex-1"
                value={shape as "continuous" | "button" | "relative"}
                options={[
                  { value: "continuous", label: "Knob / fader" },
                  { value: "button", label: "Button / pedal" },
                  { value: "relative", label: "Endless encoder" },
                ]}
                onChange={(next) =>
                  onUpdate((m) => ({
                    ...m,
                    source: { ...m.source, event: ccWithShape(event, next) },
                    behavior: defaultBehavior(suggestedBehavior(next), m.behavior),
                  }))
                }
              />
            </div>
          )}
          {stage === "action" && clash && (
            <div className="mt-3 flex items-center gap-2 rounded-lg bg-warn-soft px-2.5 py-2 text-sm text-warn">
              <span className="min-w-0 flex-1 leading-snug">
                This control already has a mapping here
                {clash.mapping.name ? ` (“${clash.mapping.name}”)` : ""}. Both would fire.
              </span>
              <Button
                size="sm"
                onClick={() => {
                  const { removeMapping, selectMapping } = useApp.getState();
                  removeMapping(mapping.id);
                  selectMapping(clash.mapping.id);
                }}
              >
                Edit that one instead
              </Button>
            </div>
          )}
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
                {primaryKind === "text"
                  ? "Now type the text it should send"
                  : primaryKind !== null && primaryKind !== "keys"
                    ? "Now choose what it should do"
                    : shape === "continuous" || shape === "relative"
                      ? "Now press the key or shortcut it should send"
                      : "Now press the key or shortcut to send"}
              </div>
              {stage === "action" && (
                <div className="text-fg-muted">
                  {primaryKind === "text"
                    ? "It is typed into whatever application is in front."
                    : primaryKind !== null && primaryKind !== "keys"
                      ? "Fill in the details below."
                      : shape === "relative"
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
              armedSlot={armedSlot}
              onCaptured={(slot) => setArmedSlot(slot + 1)}
              onChange={(behavior, key) => {
                // Choosing another behavior starts over with its first slot.
                if (behavior.type !== mapping.behavior.type) setArmedSlot(0);
                onBehavior(behavior, key);
              }}
            />
          )}
          {stage === "action" && (
            <RateTip confirm mapping={mapping} onApply={(rate_limit) => onUpdate((m) => ({ ...m, rate_limit }))} />
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
            "Rate limits, conditions, macros and more are one click away."
          )}
        </span>
        <Button onClick={onDone}>All options</Button>
        <Button
          variant="primary"
          icon={<Check size={14} />}
          disabled={!done}
          onClick={() => {
            onDone();
            useApp.getState().selectMapping(null);
            hintAfterAdding();
          }}
        >
          Done
        </Button>
      </div>
    </div>
  );
}

/**
 * The first few times a mapping is added, say where to try it: with the default settings nothing
 * is sent while this window has focus, which would otherwise look like "it does not work".
 */
function hintAfterAdding(): void {
  const state = useApp.getState();
  if (!state.config.settings.suspend_while_focused) {
    return;
  }
  let shown = 0;
  try {
    shown = Number(localStorage.getItem("m2k.hint.try-it") ?? "0");
    localStorage.setItem("m2k.hint.try-it", String(shown + 1));
  } catch {
    // Storage unavailable: just show the hint.
  }
  if (shown < 3) {
    state.toast(
      "Mapping added. Switch to the app you want to control and try it — keys are held back while this window is in front.",
      { kind: "success" },
    );
  }
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
