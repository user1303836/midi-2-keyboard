import type { Feedback } from "@/bindings/Feedback";
import type { FeedbackWhen } from "@/bindings/FeedbackWhen";
import type { Mapping } from "@/bindings/Mapping";
import { Button } from "@/components/ui/button";
import { Select } from "@/components/ui/inputs";
import { Field } from "@/components/ui/misc";
import { deviceLabel } from "@/lib/describe";
import { behaviorTypeInfo, feedbackForSource, primaryAction } from "@/lib/model";
import { useApp, useKnownLayers } from "@/lib/store";

import { InlineSwitch, LayerInput } from "./ActionEditor";
import { MidiSendEditor, OutputSelect } from "./MidiSendEditor";

/** Whether the behavior has an "on" state of its own (something held or latched, a shuttle that is repeating). */
export function hasOnState(mapping: Mapping): boolean {
  const b = mapping.behavior;
  return b.type === "hold" || b.type === "toggle" || b.type === "shuttle" || (b.type === "tap_hold" && b.hold_target !== null);
}

/**
 * The light a mapping most likely wants: on its own pad or button, showing its own state — or,
 * when what it does is switch a profile or a layer, showing that profile or layer.
 */
export function suggestFeedback(mapping: Mapping): Feedback {
  // The output is never guessed here. "The controller it came from" (null) lets the engine
  // decide at the moment of sending, and it only ever picks a controller whose input is open —
  // a suggestion must not name a device the user never chose (it would be opened and sent to).
  const device = null;
  const base: Feedback = {
    ...(feedbackForSource(mapping.source) ?? {
      when: { type: "engaged" },
      on: { type: "control_change", channel: mapping.source.channel ?? 1, controller: 1, value: { type: "fixed", value: 127 } },
      off: { type: "control_change", channel: mapping.source.channel ?? 1, controller: 1, value: { type: "fixed", value: 0 } },
    }),
    device,
  };
  const action = primaryAction(mapping.behavior);
  if (!hasOnState(mapping) && action?.type === "switch_profile" && action.target.type === "profile") {
    return { ...base, when: { type: "profile", id: action.target.id } };
  }
  if (!hasOnState(mapping) && action?.type === "layer") {
    return { ...base, when: { type: "layer", layer: action.layer } };
  }
  return base;
}

/** "Light on the controller": MIDI sent back whenever a state turns on or off. */
export function FeedbackEditor({
  mapping,
  onChange,
}: {
  mapping: Mapping;
  onChange: (feedback: Feedback | null, coalesceKey?: string) => void;
}) {
  const profiles = useApp((state) => state.config.profiles);
  const config = useApp((state) => state.config);
  const outputs = useApp((state) => state.outputs);
  const layers = useKnownLayers();
  const feedback = mapping.feedback;

  if (!feedback) {
    const suggested = suggestFeedback(mapping);
    // A light that follows "this mapping is on" can never show for a behavior with no on state.
    const stateless = !hasOnState(mapping) && suggested.when.type === "engaged";
    const selectable = profiles.find((p) => !p.always_on)?.id ?? profiles[0]?.id ?? "";
    return (
      <div className="flex flex-col gap-2">
        {!stateless && <InlineSwitch label="Send MIDI back to show a state" checked={false} onChange={() => onChange(suggested)} />}
        <div className="text-sm leading-snug text-fg-faint">
          Lights a pad while a toggle or a layer is on, or shows which profile is active. The controller has
          to react to MIDI it receives — most pad controllers light a pad for “note on” and switch it off for
          velocity 0.
        </div>
        {stateless && (
          <div className="rounded-md border bg-app p-2.5 text-sm leading-snug text-fg-muted">
            “{behaviorTypeInfo(mapping.behavior.type).label}” has no on / off state of its own, so there is nothing for its
            light to show. Give this mapping the behavior <b className="text-fg">Hold</b> (lit while held) or{" "}
            <b className="text-fg">Toggle</b> (on and off with each press) — or light it for something else:
            <div className="mt-2 flex flex-wrap gap-2">
              <Button size="sm" onClick={() => onChange({ ...suggested, when: { type: "layer", layer: layers[0] ?? "shift" } })}>
                While a layer is on…
              </Button>
              <Button size="sm" disabled={!selectable} onClick={() => onChange({ ...suggested, when: { type: "profile", id: selectable } })}>
                While a profile is active…
              </Button>
            </div>
          </div>
        )}
      </div>
    );
  }

  const set = (change: Partial<Feedback>, key?: string) => onChange({ ...feedback, ...change }, key);
  const when = feedback.when;
  // Where the light will go, as far as that can be known before anything is touched.
  const target = feedback.device ?? mapping.source.device;
  const offIsVelocityZero =
    feedback.off.type === "note_on" && feedback.off.velocity.type === "fixed" && feedback.off.velocity.value === 0;

  return (
    <div className="flex flex-col gap-3">
      <InlineSwitch label="Send MIDI back to show a state" checked onChange={() => onChange(null)} />
      <Field label="Shows">
        <div className="flex flex-wrap items-center gap-1.5">
          <Select<FeedbackWhen["type"]>
            aria-label="What the light shows"
            className="w-[212px]"
            value={when.type}
            options={[
              { value: "engaged", label: "This mapping is on", hint: mapping.behavior.type === "shuttle" ? "while it repeats" : "held, or toggled on" },
              { value: "layer", label: "A layer is on" },
              { value: "profile", label: "A profile is active" },
            ]}
            onChange={(type) =>
              set({
                when:
                  type === "engaged"
                    ? { type }
                    : type === "layer"
                      ? { type, layer: layers[0] ?? "shift" }
                      : { type, id: profiles.find((p) => !p.always_on)?.id ?? "" },
              })
            }
          />
          {when.type === "layer" && <LayerInput value={when.layer} onChange={(layer) => set({ when: { type: "layer", layer } }, "layer")} />}
          {when.type === "profile" && (
            <Select
              aria-label="Profile the light shows"
              className="w-[160px]"
              value={when.id}
              options={profiles.filter((p) => !p.always_on || p.id === when.id).map((p) => ({ value: p.id, label: p.name }))}
              onChange={(id) => set({ when: { type: "profile", id } })}
            />
          )}
        </div>
      </Field>
      {when.type === "engaged" && !hasOnState(mapping) && (
        <div className="rounded-md bg-warn-soft px-2.5 py-1.5 text-sm leading-snug text-warn">
          “{behaviorTypeInfo(mapping.behavior.type).label}” is never on for longer than an instant, so this light would
          never show. Use Hold or Toggle — or let the light show a layer or a profile instead.
        </div>
      )}
      <Field
        label="When on"
        align="start"
        hint={feedback.on.type === "note_on" ? "On many pad controllers the velocity picks the colour." : undefined}
      >
        <MidiSendEditor message={feedback.on} allowIncoming={false} onChange={(on, key) => set({ on }, key && `on:${key}`)} />
      </Field>
      <Field label="When off" align="start" hint={offIsVelocityZero ? "“Note on” with velocity 0 is what switches most pads off." : undefined}>
        <MidiSendEditor message={feedback.off} allowIncoming={false} onChange={(off, key) => set({ off }, key && `off:${key}`)} />
      </Field>
      <Field
        label="Send to"
        align="start"
        hint={
          outputs.length === 0
            ? "No MIDI output is connected right now — the light shows as soon as one that fits is there."
            : target === null
              ? "This mapping listens to any device: the light goes to the only connected controller that accepts MIDI — with several, to the one its control was last touched on. Pick one here to be sure."
              : !outputs.some((port) => port.name === target)
                ? `There is no MIDI output called “${deviceLabel(target, config)}” right now — the light shows once it is there.`
                : undefined
        }
      >
        <OutputSelect className="w-[260px]" value={feedback.device} onChange={(device) => set({ device })} />
      </Field>
    </div>
  );
}
