import type { MidiSend } from "@/bindings/MidiSend";
import type { SendValue } from "@/bindings/SendValue";
import { NumberInput, Segmented, Select, TextInput, type SelectOption } from "@/components/ui/inputs";
import { deviceLabel } from "@/lib/describe";
import { midiBytesProblem } from "@/lib/midi";
import { defaultMidiSend, MIDI_SEND_TYPES, type MidiSendType } from "@/lib/model";
import { useApp } from "@/lib/store";

import { NoteInput } from "./fields";

const CHANNELS: SelectOption<string>[] = Array.from({ length: 16 }, (_, index) => ({
  value: String(index + 1),
  label: `Channel ${index + 1}`,
}));


function ValueField({
  label,
  value,
  onChange,
  allowIncoming,
}: {
  label: string;
  value: SendValue;
  onChange: (value: SendValue, coalesceKey?: string) => void;
  /** Offer "the value of the message that triggered this" (pointless where no message is at hand). */
  allowIncoming: boolean;
}) {
  return (
    <span className="flex items-center gap-1.5">
      <span className="text-sm text-fg-muted">{label}</span>
      {allowIncoming && (
        <Segmented<SendValue["type"]>
          size="sm"
          value={value.type}
          options={[
            { value: "fixed", label: "Fixed" },
            { value: "incoming", label: "As received" },
          ]}
          onChange={(type) => onChange(type === "fixed" ? { type: "fixed", value: 127 } : { type: "incoming" })}
        />
      )}
      {value.type === "fixed" && (
        <NumberInput
          aria-label={label}
          className="w-[64px]"
          min={0}
          max={127}
          value={value.value}
          onChange={(next) => onChange({ type: "fixed", value: next }, "value")}
        />
      )}
    </span>
  );
}

/** Picks the MIDI output a message goes to. `null` is "the controller itself". */
export function OutputSelect({
  value,
  onChange,
  className,
}: {
  value: string | null;
  onChange: (value: string | null) => void;
  className?: string;
}) {
  const outputs = useApp((state) => state.outputs);
  const config = useApp((state) => state.config);
  const names = outputs.map((port) => port.name);
  if (value && !names.includes(value)) names.push(value);
  const options: SelectOption<string>[] = [
    { value: "\u0000same", label: "The controller it came from", hint: "the output with the same name as the input" },
    ...names.map((name) => ({
      value: name,
      label: deviceLabel(name, config),
      hint: outputs.some((port) => port.name === name) ? undefined : "not connected",
    })),
  ];
  return (
    <Select
      aria-label="MIDI output"
      className={className}
      value={value ?? "\u0000same"}
      onChange={(next) => onChange(next === "\u0000same" ? null : next)}
      options={options}
    />
  );
}

/** "To": the output a message goes to, with a word when there is nowhere for it to go. */
export function OutputField({ value, onChange }: { value: string | null; onChange: (value: string | null) => void }) {
  const none = useApp((state) => state.outputs.length === 0);
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-1.5">
        <span className="shrink-0 text-sm text-fg-muted">To</span>
        <OutputSelect className="min-w-0 flex-1" value={value} onChange={onChange} />
      </div>
      {none && (
        <span className="text-sm leading-snug text-fg-faint">
          No MIDI output is connected right now. This is sent as soon as one that fits is there.
        </span>
      )}
    </div>
  );
}

/** Edits one MIDI message to send: its kind, channel, number and value. */
export function MidiSendEditor({
  message,
  onChange,
  allowIncoming = true,
}: {
  message: MidiSend;
  onChange: (message: MidiSend, coalesceKey?: string) => void;
  allowIncoming?: boolean;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex flex-wrap items-center gap-1.5">
        <Select<MidiSendType>
          aria-label="MIDI message"
          className="w-[150px]"
          value={message.type}
          options={MIDI_SEND_TYPES.map((info) => ({ value: info.type, label: info.label, hint: info.hint }))}
          onChange={(type) => onChange(defaultMidiSend(type, message))}
        />
        {"channel" in message && (
          <Select
            aria-label="Channel to send on"
            className="w-[112px]"
            value={String(message.channel)}
            options={CHANNELS}
            onChange={(channel) => onChange({ ...message, channel: Number(channel) })}
          />
        )}
      </div>
      <div className="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        {(message.type === "note_on" || message.type === "note_off") && (
          <span className="flex items-center gap-1.5">
            <span className="text-sm text-fg-muted">Note</span>
            <NoteInput aria-label="Note to send" value={message.note} onChange={(note) => onChange({ ...message, note })} />
          </span>
        )}
        {message.type === "note_on" && (
          <ValueField
            label="Velocity"
            value={message.velocity}
            allowIncoming={allowIncoming}
            onChange={(velocity, key) => onChange({ ...message, velocity }, key && "velocity")}
          />
        )}
        {message.type === "control_change" && (
          <>
            <span className="flex items-center gap-1.5">
              <span className="text-sm text-fg-muted">Controller</span>
              <NumberInput
                aria-label="Controller to send"
                className="w-[64px]"
                min={0}
                max={127}
                value={message.controller}
                onChange={(controller) => onChange({ ...message, controller }, "controller")}
              />
            </span>
            <ValueField
              label="Value"
              value={message.value}
              allowIncoming={allowIncoming}
              onChange={(value, key) => onChange({ ...message, value }, key && "ccvalue")}
            />
          </>
        )}
        {message.type === "program_change" && (
          <ValueField
            label="Program"
            value={message.program}
            allowIncoming={allowIncoming}
            onChange={(program, key) => onChange({ ...message, program }, key && "program")}
          />
        )}
        {message.type === "raw" && (
          <div className="flex min-w-0 flex-1 flex-col gap-1">
            <TextInput
              aria-label="Bytes to send"
              mono
              placeholder="Hex bytes, e.g. F0 7F 7F 06 02 F7"
              value={message.bytes}
              onChange={(bytes) => onChange({ ...message, bytes }, "bytes")}
            />
            {midiBytesProblem(message.bytes) && <span className="text-sm text-warn">{midiBytesProblem(message.bytes)}</span>}
          </div>
        )}
      </div>
    </div>
  );
}
