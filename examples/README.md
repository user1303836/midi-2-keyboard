# Example profiles

Ready-made profiles you can import into midi-2-keyboard and adapt. Each file is a normal profile
export (`{"format": "midi-2-keyboard-profiles", ...}`), so the app's **Import** reads it like
any other export. Every mapping listens to **any device** and **any channel** unless noted, so
they work with whatever controller is plugged in. Change the notes and controller numbers to
the ones your hardware sends: MIDI learn in the editor fills them in for you.

| File | Who it is for | What it shows | Controls to touch |
|---|---|---|---|
| `daw-endless-encoders.json` | Producers with an endless-encoder box (Faderfox EC4 style) | Relative encoders in all three encodings, acceleration kept (fast turns send several steps), a throttled zoom, tap vs. hold on an encoder's push switch, a "fine mode" layer | Encoders on CC 16 / 17 / 18 set to relative *two's complement*, *binary offset* and *sign magnitude*; push switches on CC 48 / 49 |
| `video-jog-shuttle.json` | Video editors | Jog encoder = frame steps; spring-loaded shuttle knob = hold J / K / L with hysteresis so it does not flicker at the edges; mark in / out; the pitch wheel walks the timeline frame by frame, faster the further it is pushed | Jog on CC 16 (relative), shuttle knob on CC 20 (centred at 64), pads on notes 36–38, the pitch wheel |
| `streamer-pads.json` | Streamers | An always-on push-to-talk and mute, OBS scene pads that are only active while OBS has focus, the same pads doing game actions while a game window has focus, a latching button as an open mic | Pads on notes 36, 37, 40–43; latching button on CC 85. Edit the app rules: OBS matches `obs*`, the game profile matches windows titled `*Game*` |
| `gaming-keyboard-wasd.json` | Gamers with a MIDI keyboard | Keys held as WASD exactly as long as the piano keys, a chord (C–E–G) as jump, the mod wheel as sprint, the pitch wheel as look up / down — all with hysteresis | C4 D4 E4 F4, C3+E3+G3 together, mod wheel, pitch wheel |
| `drum-pads-velocity.json` | Drummers, finger drummers | Velocity zones (soft vs. hard hit), auto-fire while a pad is held, mouse click and drag from pads, aftertouch "squeeze" as a modifier | Pads on notes 36, 38, 42, 49; pressing a pad down after hitting it (poly aftertouch) |
| `presenter-footswitch.json` | Presenters, teachers | One foot switch: press once = next slide, twice = previous, three times = black screen; a second pedal with tap vs. long press | Sustain pedal (CC 64), soft pedal (CC 67) |
| `accessibility-pedals.json` | Anyone who wants modifiers or keys on their feet | Sustain pedal = Shift while held, a pedal that latches Ctrl, a pedal with reversed polarity, an expression pedal as push-to-talk for dictation, squeezing the keys as Space, a "release everything" key | CC 64, 67, 66, 11, channel aftertouch, the lowest key (A0) |
| `live-show-control.json` | Live performers | Program changes select song profiles, MIDI clock presses a key on every bar, MMC transport and MIDI Stop drive media keys / the backing video | Program changes 0 and 1 (patches 1 and 2 on most devices), MIDI clock from the DAW or sequencer, MMC Play / Stop, pad on note 36 |
| `coder-macro-pad.json` | Programmers | A text snippet with placeholders (`{note_name}`, `{velocity}`), a multi-step command-palette macro, tap = Esc / hold = Ctrl, an Fn layer that gives two pads a second meaning | Pads on notes 36–41 (hold 39 for the Fn layer) |
| `nrpn-and-14-bit.json` | Synth and high-resolution controller owners | A 14-bit NRPN knob stepping in fine increments, NRPN data increment / decrement buttons, a 14-bit fader (CC 7 + 39) giving one step per 1/16 of its travel, reacting to an RPN change | NRPN 130 and 131, CC 7 / 39 as a 14-bit pair, RPN 0 |
| `pad-lights.json` | Anyone whose pad controller has lights | Pads that light up to show state: three latches (auto-run W, crouch C, open mic V) lit while on; a "shift" pad lit while held, which turns the next pad's Screenshot (F12) into Save a clip (F9); two page pads switching between a media page and a volume page, only the active page's pad lit | Pads on notes 36–38 (latches), 40 (shift), 41, 42 / 43 (pages), 44 / 45 (page 1: play / pause, next track; page 2: volume up / down) |
| `knobs-and-odd-buttons.json` | Everyone with knobs and cheap buttons | The classic "knob CC 20 on channel 2 fires Ctrl+Shift+O at most every 500 ms", a volume knob, "type the value once the knob stops", buttons that only ever send 127 or that alternate 127 / 0 (both fire on every press), a crossfader split into three zones | CC 20 on channel 2, CC 7, 21, 80, 81, 8 |

## Notes

- **Buttons that send only "on" (127) and never "off".** A mapping that reacts to the button
  being *pressed* fires only the first time, because the control never reports a release. Use
  "on every message" (`change`) instead, as `knobs-and-odd-buttons.json` does.
- **Encoder speed.** Encoder steps use a short key tap (`tap_ms: 5`) so that fast turns are not
  delayed behind a queue of long key presses.
- **Per-app profiles** need the name of your application: open the profile and pick it from the
  list of running applications.
- **Lights need a controller that reacts to the notes it receives** — most pad controllers light a
  pad for a note on and switch it off for velocity 0 (on many the velocity picks the colour).
  `pad-lights.json` sends velocity 127 / 0 on **channel 10**; change the channel and velocities in
  each light to what your controller expects. A light goes to the controller its pad is pressed
  on: with one controller connected, the lights show from the start; with several, a pad lights
  up only after its first press, unless you pick the controller under *Send to*.
- **Combining examples.** "Streaming: voice", "Live show: control" and "Lights: pad grid" are
  *always on*: their mappings work in every profile. They use pads (notes 36 and up) that other
  examples use too, so after importing them together those pads do two things at once. The list
  marks such mappings as **shared**; change the notes on one side.
