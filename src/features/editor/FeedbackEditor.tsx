import type { Feedback } from "@/bindings/Feedback";
import type { FeedbackWhen } from "@/bindings/FeedbackWhen";
import type { Mapping } from "@/bindings/Mapping";
import { Select } from "@/components/ui/inputs";
import { Field } from "@/components/ui/misc";
import { behaviorTypeInfo, feedbackForSource, primaryAction } from "@/lib/model";
import { useApp } from "@/lib/store";

import { InlineSwitch, LayerInput } from "./ActionEditor";
import { MidiSendEditor, OutputSelect } from "./MidiSendEditor";

/** Whether the behavior has an "on" state of its own (something held or latched). */
function hasOnState(mapping: Mapping): boolean {
  const b = mapping.behavior;
  return b.type === "hold" || b.type === "toggle" || (b.type === "tap_hold" && b.hold_target !== null);
}

/**
 * The light a mapping most likely wants: on its own pad or button, showing its own state — or,
 * when what it does is switch a profile or a layer, showing that profile or layer.
 */
export function suggestFeedback(mapping: Mapping): Feedback {
  const base: Feedback = feedbackForSource(mapping.source) ?? {
    device: null,
    when: { type: "engaged" },
    on: { type: "control_change", channel: mapping.source.channel ?? 1, controller: 1, value: { type: "fixed", value: 127 } },
    off: { type: "control_change", channel: mapping.source.channel ?? 1, controller: 1, value: { type: "fixed", value: 0 } },
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
  const feedback = mapping.feedback;

  if (!feedback) {
    return (
      <div className="flex flex-col gap-2">
        <InlineSwitch label="Send MIDI back to show a state" checked={false} onChange={() => onChange(suggestFeedback(mapping))} />
        <div className="text-sm leading-snug text-fg-faint">
          Lights a pad while a toggle or a layer is on, or shows which profile is active. The controller has
          to react to MIDI it receives — most pad controllers light a pad for “note on” and switch it off for
          velocity 0.
        </div>
      </div>
    );
  }

  const set = (change: Partial<Feedback>, key?: string) => onChange({ ...feedback, ...change }, key);
  const when = feedback.when;

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
              { value: "engaged", label: "This mapping is on", hint: "held, or toggled on" },
              { value: "layer", label: "A layer is on" },
              { value: "profile", label: "A profile is active" },
            ]}
            onChange={(type) =>
              set({
                when:
                  type === "engaged"
                    ? { type }
                    : type === "layer"
                      ? { type, layer: "shift" }
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
      <Field label="When on" align="start">
        <MidiSendEditor message={feedback.on} allowIncoming={false} onChange={(on, key) => set({ on }, key && `on:${key}`)} />
      </Field>
      <Field label="When off" align="start">
        <MidiSendEditor message={feedback.off} allowIncoming={false} onChange={(off, key) => set({ off }, key && `off:${key}`)} />
      </Field>
      <Field label="Send to">
        <OutputSelect className="w-[260px]" value={feedback.device} onChange={(device) => set({ device })} />
      </Field>
    </div>
  );
}
