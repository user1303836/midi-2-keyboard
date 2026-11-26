# MIDI 2 Keyboard

A fully featured, fully customizable application for translating MIDI events to keyboard events
(and mouse events, text, macros, …). Windows, macOS and Linux.

It is built around two promises:

- **The simple case is trivial.** Click **Add mapping**, press middle C on your controller, press
  the Up arrow on your keyboard. Done.
- **The weird case is possible.** *"CC 20 on channel 2 comes from a knob; when I turn it, send
  Ctrl+Shift+O — but at most once every 500 ms."* That is one mapping with a rate limit.

## What it can do

**Every kind of MIDI input**

| Source | Notes |
| --- | --- |
| Notes | single notes or ranges, any/one channel, velocity zones |
| Chords | several notes held together, optionally "struck within N ms" |
| Control Change | knobs/faders, buttons and pedals (a "pressed" range), endless encoders in all three relative encodings (two's complement, binary offset, sign-magnitude) |
| 14-bit CC, NRPN, RPN | assembled from their MSB/LSB/data messages; 7-bit or 14-bit data; increment/decrement |
| Program Change, Pitch Bend, Channel and Poly Pressure | |
| SysEx | byte patterns with `??` (any byte) and `**` (any run) wildcards, MMC presets |
| Clock, Start/Stop/Continue, Song Select/Position, … | e.g. "every beat" or "every bar" of MIDI clock |

**MIDI learn that understands the control.** Touch something and the app works out whether it is a
key, a pad, a button, a pedal, an absolute knob, or an endless encoder (and which encoding it
speaks), or an NRPN parameter.

**Behaviors** — how a control is interpreted:

- **Press** – fire once on press (and, optionally, on release); optional auto-fire while held
- **Hold** – keep keys down exactly as long as the control is held, with keyboard-like auto-repeat
- **Toggle** – first press holds, next press releases
- **On change** – fire whenever a knob/fader moves
- **Up / Down** – one action per step up, another per step down (absolute controls and encoders)
- **Tap or hold** – different actions for a quick tap and a long press (including dual-role keys:
  tap = Esc, hold = Ctrl)
- **Multi-tap** – single / double / triple press
- **Cycle** – each press fires the next action in a list

**Actions** — key combinations, typed text (with `{value}`, `{note_name}`, … placeholders), mouse
clicks / scrolling / movement, macros with delays, profile switching, layers, pause/resume/panic,
launching programs, opening files and URLs.

**Shaping** — rate limits (throttle, throttle with a final trailing fire, debounce), conditions
(only in a certain application, only while a layer is on, only while another pad is held, only
while a knob is in a certain position), hysteresis for jittery pedals and faders, per-action
timing.

**Organization** — profiles, always-on (global) profiles, automatic profile switching by
foreground application (picked from a list of open apps), import/export, undo/redo. The list
points out mappings that share a control.

**Feedback** — a live monitor of everything that comes in and everything that goes out (including
*why* something was not sent), the engine's measured MIDI-to-key latency, an activity light on
every mapping, and an on-screen virtual controller for trying things without hardware.

**Safety** — everything the app holds down is tracked and released on pause, profile switch,
device disconnect, config change and exit; a key release the system refuses is retried until it
goes through. By default nothing is sent while the app's own window has focus, so a mapping
cannot type into (or close) the editor.

**Light on the machine** — the engine is native code on its own thread (tens of microseconds
from MIDI message to key event, 0% CPU when idle). Closing the window keeps the mappings running
from the tray with no browser view loaded at all (about 30 MB).

## Platform status

| | Builds and packages in CI | Verified on a real machine |
| --- | --- | --- |
| Windows 10/11 | yes (NSIS + MSI) | yes — end to end, real keystrokes |
| macOS 11+ (universal) | yes (DMG) | not yet — compiled and reviewed only |
| Linux (X11 / Wayland via uinput) | yes (deb, rpm, AppImage) | not yet — compiled and reviewed only |

## Using it

1. Start the app. Connected MIDI devices are opened automatically (bottom left).
2. Click **Add mapping** (or press `N`), touch a control, press the key or shortcut to send.
3. Switch to the application you want to control and use your controller.

Everything else is under **All options** in the editor. The **Monitor** (bottom right, `Ctrl+M`)
shows what is going on, and its *Virtual controller* tab lets you play notes and turn knobs
without any hardware. Ready-made profiles to import and pick apart are in [`examples/`](examples).

Keys are not sent while the app's own window is in front (so a mapping cannot type into the
editor) — switch to the application you want to control to try a mapping. Closing the window
keeps the app running in the tray; quit from the tray icon's menu.

Good to know:

- **Windows:** most MIDI drivers allow only one program to open a device at a time. If your DAW has
  the controller open, this app cannot open it (and vice versa) — it will say so and keep retrying.
  To control windows that run as administrator, run this app as administrator too.
- **macOS:** the app needs the *Accessibility* permission to send keys
  (System Settings → Privacy & Security → Accessibility). It asks on first use.
- **Linux:** on X11 it works out of the box. On Wayland it needs access to `/dev/uinput`: the deb
  and rpm packages install the udev rule for that; with the AppImage the app shows the rule to
  add. Per-application profiles need X11.

The configuration is a single JSON file (Settings → *Show config file*), saved atomically with a
backup copy; a corrupt file is set aside and the backup restored. You can edit it by hand while
the app is running.

## Building from source

Prerequisites: [Rust](https://rustup.rs) (stable) and Node.js 20+.

- **Windows:** the MSVC toolchain (Visual Studio Build Tools, "Desktop development with C++"), or
  the GNU toolchain with MinGW-w64 on `PATH` (`windres` and `dlltool` are needed).
- **macOS:** Xcode command line tools.
- **Linux (Debian/Ubuntu):** `sudo apt install build-essential pkg-config libssl-dev libasound2-dev
  libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev libxdo-dev`

```sh
npm install
npm run tauri dev        # run the desktop app in development mode
npm run tauri build      # build a release bundle into target/release/bundle
```

Other useful commands:

```sh
npm run dev                                   # the UI alone in a browser, against a mock backend
npm run bindings                              # regenerate the TypeScript types from the Rust types

# m2k, the headless companion (npm run cargo -- run -p m2k-cli -- <command>)
m2k devices                                   # list MIDI input ports
m2k monitor [name]                            # print incoming MIDI, decoded
m2k run [--config file] [--dry-run]           # run a config without the UI (dry run sends nothing)
m2k simulate config.json script.txt           # replay scripted MIDI and print what would be sent
```

## How it is put together

```
crates/m2k-midi     MIDI parsing, state tracking (NRPN/RPN/14-bit), device input with hot-plug
crates/m2k-output   key model and OS input injection (SendInput / CGEvent / uinput / XTest)
crates/m2k-core     config model, persistence, the engine, the application runtime
crates/m2k-cli      headless tools (devices, monitor, simulate)
src-tauri           the desktop shell (Tauri v2): commands, tray, autostart, single instance
src                 the UI (React + TypeScript)
```

The engine runs on its own thread with a timer queue and never touches the UI thread: a MIDI
message goes from the OS callback through a channel into the engine and out through the OS input
API. The UI only ever sees batched copies of what happened.

Contributor notes for people and agents are in [AGENTS.md](AGENTS.md).
