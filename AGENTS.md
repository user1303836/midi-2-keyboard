# AGENTS.md — midi-2-keyboard

Instructions for every agent (and human) working in this repository. Read this first.

## Ground rules

1. **Do NOT write tests.** No unit tests, no integration tests, no test harnesses, no `#[cfg(test)]`
   modules, no `*.test.ts` files, no test dependencies. This is a greenfield project with no users;
   the owner wants fast iteration and will test manually (or have agents exercise the running app).
   Verify your work by building and *running* the real thing, not by writing test code.
2. **Push directly to `main`.** Pull requests are disabled on this repository. No feature branches
   left on the remote, no PRs. Commit small, coherent, *building* increments and push them.
   Always `git pull --rebase origin main` before `git push origin HEAD:main`.
3. **The repository owner is the sole contributor.** Commit with the git identity already configured
   on this machine and nothing else. Never add `Co-Authored-By:` trailers, "Generated with ..." lines,
   or any other attribution to commits, files, or docs. Never change `user.name` / `user.email`.
4. **Never push a broken build.** `npm run cargo -- check` (the core crates) and, if you touched
   the frontend, `npm run typecheck` must pass before you push. If you touched `src-tauri`, also
   `npm run build && npm run cargo -- check -p midi-2-keyboard`.
5. **Don't create planning / analysis / status markdown files.** Durable knowledge goes in this file
   or `README.md`; everything else stays in the conversation.

## What we are building

A standalone, performant, fully featured, fully customizable desktop application that translates
MIDI events into keyboard (and mouse) events. Windows, macOS, and Linux.

Two things must both be true at all times:

- **The simple case is trivial.** "Middle C presses the Up arrow" is: click *Add*, hit the note,
  press the key. Two gestures, no forms.
- **The weird case is possible.** Example of the expected granularity: *CC 20 on channel 2, from a
  knob, fires Ctrl+Shift+O — but at most once per 500 ms.* Think throttles, debounces, relative
  encoders, velocity ranges, NRPN/RPN, SysEx patterns, chords, long-press vs tap, toggles, per-app
  profiles, layers, macros. When you add a feature, go looking for its edge cases (stuck keys on
  disconnect, running status, note-on with velocity 0, 14-bit pairs, hysteresis around a threshold,
  two mappings holding the same key, ...) and handle them.

**UI/UX is paramount.** The app can be extremely granular, so the interface has to make that depth
approachable: progressive disclosure, sentence-like summaries of every mapping, MIDI learn and key
capture everywhere, sensible defaults, clear live feedback. Never ship a raw form dump.

Performance and stability matter more than cleverness: the MIDI → key path must stay off the UI
thread, must never block, and must never leave keys stuck down.

## Architecture

```
crates/
  m2k-midi/     MIDI domain: message types, byte-stream parser, NRPN/RPN/14-bit state tracking,
                device I/O (midir) with hot-plug handling: inputs (`input.rs`) and outputs
                (`output.rs`: ports opened on first send, a send that never blocks).
  m2k-output/   Key model + OS input injection (keyboard, mouse, unicode text) per platform,
                held-key bookkeeping, platform probes (permissions, foreground app).
  m2k-core/     Config model (the JSON schema), persistence, and the engine: matching,
                behaviors (trigger/hold/toggle/tap-hold/multi-tap/step/...), rate limiting,
                scheduling, conditions, profiles, monitor events, and MIDI going back out
                ("Send MIDI" actions, feedback lights) through a `MidiSink`. `runtime.rs` wires
                config + engine + MIDI input and output together for any front end.
  m2k-cli/      `m2k` headless tools: devices, outputs, send, monitor, run, simulate,
                export-bindings.
src-tauri/      Tauri v2 desktop shell: commands/events between the runtime and the UI, tray,
                autostart, single-instance. While the app sits in the tray there is no window
                (and no webview) at all; it is rebuilt on demand.
examples/       Importable example profiles.
src/            React + TypeScript + Vite frontend.
src/bindings/   TypeScript types GENERATED from the Rust types with ts-rs. Never edit by hand.
```

- The engine runs on its own thread with a timer queue. It never sleeps inside an action and
  never blocks on the UI. The UI talks to it only through Tauri commands and batched events.
- **Rust types are the single source of truth** for config and IPC. After changing any type that
  derives `TS`, regenerate bindings with `npm run bindings` and commit the result.
- **Conventions that must not drift:**
  - MIDI channels are **1–16** everywhere (config, API, UI). Index arrays with `channel - 1`.
  - Notes, controllers, 7-bit values are 0–127. 14-bit values are 0–16383. Pitch bend is 0–16383
    with 8192 as center.
  - Keys are named after the W3C `KeyboardEvent.code` values (`KeyA`, `ArrowUp`, `ControlLeft`,
    `F13`, `NumpadAdd`, `AudioVolumeUp`). They denote physical keys.
  - Config changes must stay backward compatible: add fields with `#[serde(default)]`; never
    rename or remove without a migration in `m2k-core`. A hand-written config may leave out
    everything that has an obvious value (see "Writing the config by hand" in the README); keep
    it that way when adding fields.
  - A controller's output is found **by name**: the output port named like the input the message
    came from. Port names go through the same normalization on both sides.
  - An empty action (no keys, no text, a macro without effect) is no action: the engine never
    fires or reports it.
  - A shuttle's arithmetic exists twice: in the engine and, for the editor's live preview and
    the mock, in `src/lib/model.ts` (`shuttleRestZone`, `shuttleAt`, `shuttleRate`). The doc
    comment on `Behavior::Shuttle` is the contract; change both sides together.
- Platform-specific code lives behind `cfg` in per-OS modules. Every platform must keep compiling;
  CI builds all three.

## Building and running

Prerequisites: Rust (stable), Node 20+. Platform extras:

- **Windows:** the MSVC toolchain *or* the GNU toolchain plus MinGW-w64 (`windres` is needed for
  the app resources). On this dev machine the GNU toolchain is the default and a portable
  MinGW lives in the git-ignored `.tools/mingw64`; the npm scripts add it to `PATH` automatically.
- **Linux:** `libasound2-dev`, `libwebkit2gtk-4.1-dev`, `libxdo-dev`/X11 libs, `libayatana-appindicator3-dev`,
  `librsvg2-dev`, `pkg-config`, `build-essential`.
- **macOS:** Xcode command line tools.

```
npm install              # once
npm run dev              # UI only, in a browser, against the mock backend (fast UI iteration)
npm run tauri dev        # the real desktop app
npm run tauri build      # release bundle
npm run typecheck        # tsc
npm run bindings         # regenerate src/bindings from Rust types
npm run cargo -- check   # Rust compile check of the core crates (cargo with the build tools on PATH)
npm run cargo -- run -p m2k-cli -- devices   # the headless CLI (devices / monitor / simulate)
```

`npm run cargo -- <args>` and `npm run tauri -- <args>` are plain `cargo` / `tauri` wrapped by
`scripts/with-tools.mjs`, which puts `.tools/mingw64/bin` on `PATH` when it exists. The Tauri shell
(`src-tauri`) is not a default workspace member: it needs the built frontend (`npm run build`) to
compile, so plain `cargo check` skips it.

### Checking Linux and macOS from the Windows dev machine

CI builds all three platforms, but `cfg`-gated code can be type-checked locally before pushing
(check / clippy only: nothing is linked or run). The rustup targets `x86_64-unknown-linux-gnu`
and `aarch64-apple-darwin` are installed on this machine.

```
npm run cargo -- check --target aarch64-apple-darwin        # macOS: works as is
npm run cargo -- check --target x86_64-unknown-linux-gnu    # Linux: needs the stub below
```

For Linux, `alsa-sys`'s build script runs `pkg-config`, which refuses to cross-compile. Give it
a stub: a file `stub-pkg-config.cmd` *outside the repository* containing the single line
`@echo -lasound`, then (bash) `PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG='C:\path\to\stub-pkg-config.cmd'`
before the command (PowerShell: set `$env:PKG_CONFIG_ALLOW_CROSS = "1"` and `$env:PKG_CONFIG`
first). This covers the core crates (the default members, also with `clippy -p <crate>`); the
Tauri shell needs the target's system libraries to build, so its builds are CI's. (Its code can
still be type-checked from here: with stub `cc` and `ar` next to the stub `pkg-config`,
`clippy -p midi-2-keyboard --target …` works for both targets.)

## Verifying without tests

There is no test suite (see the ground rules). Verify by exercising the real thing:

- **Engine semantics:** `npm run cargo -- run -p m2k-cli -- simulate <config.json> <script>` runs a
  config against scripted, timed MIDI through the real engine with a logging backend and prints
  every key event and engine event. `m2k run --dry-run` does the same against the real MIDI
  ports. Neither presses real keys.
- **The UI alone:** `npm run dev` serves it against a mock backend; `window.__m2k.send(device,
  bytes)` in the page plays MIDI into the mock, and `?select=<mapping-id>`, `?wizard=1`,
  `?monitor=1`, `?panel=settings|devices`, `?theme=light|dark` put it into a given state.
- **The real desktop app, end to end (Windows):** start the built exe with
  `M2K_CONFIG_DIR=<scratch dir>` (never touch the user's real config) and
  `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=<port>`, then drive the webview
  over the Chrome DevTools Protocol (e.g. `playwright-core`'s `connectOverCDP`, kept outside the
  repo). `window.__TAURI_INTERNALS__.invoke("send_midi", { device, bytes })` feeds MIDI exactly
  like a device would (one call may carry many messages), `invoke("snapshot")` returns config +
  live status. To check real output, open a Notepad window of your own, make sure **it** is the
  foreground window before every send, and read its text back (`WM_GETTEXT`). The app holds
  output back while its own window has focus.
- The scripts that drove the app during the build sessions are kept on the dev machine in the
  git-ignored `.agents/verify-scripts/` (outside version control on purpose: the repository has
  no tests). `pw/*.mjs` drive the browser mock (`npm run dev` running, then `node pw/<name>.mjs`)
  or, for `e2e`, `e2e2`, `shell`, `reload`, `examples`, `profiles`, `realtour` and everything
  ending in `-real`, the real app (`M2K_EXE=<path to the exe>` and `M2K_NO_MIDI_OUT=1`; only
  `midiout-real` and `passon-real` need MIDI output, and they name nothing but the GS synth).
  `sim/` holds `m2k simulate` scenarios, `useridle.ps1` prints how long the desktop has been
  idle. Reuse and extend them rather than starting over: they are the only regression net there
  is. Scripts written against the mock check the interface, not the engine.
- This injects real input into the owner's desktop: only ever into a window you opened, keep
  runs short, and make sure nothing is left held (`snapshot().engine.held_keys` must be empty).
- **Never send MIDI to the owner's hardware.** A controller that receives MIDI changes what it
  shows or stores, and other gear may hang off an interface's outputs. Engine-side checks of
  "Send MIDI" and lights belong in `m2k simulate` (it prints `MIDI-OUT` lines; a dry run has no
  output at all). **Start every real app and every `m2k run` with `M2K_NO_MIDI_OUT=1`**: with it
  set (to anything) nothing is ever sent, lights and "Send MIDI" report "MIDI output is switched
  off" and the outputs are still listed. A light needs no explicit device to reach a controller:
  it goes to the output named like the input, and that once sent two messages to the owner's
  controller from a test profile. The only run without the variable is one that is about the
  real output path itself: on Windows, name "Microsoft GS Wavetable Synth" as the output and
  send only control changes (silent); never a note, the machine has speakers.
- The owner may be sitting at the machine. A run that takes the foreground or sends real input
  steals their keystrokes (one of theirs once ended up as the key of a test mapping) and can put
  yours into their window. Before every such run, read how long the desktop has been idle
  (`GetLastInputInfo` from user32 against `GetTickCount` from kernel32, not
  `Environment.TickCount`, which counts from somewhere else) and hold off while somebody is
  typing; your own injected input resets that clock, so check before you start, not during.
- Only one instance of the app runs per login session (the single-instance guard), **across
  agents too**: while somebody else's instance is up, your launch just hands over to theirs and
  exits, and your script never connects. Look first (`Get-CimInstance Win32_Process -Filter
  "Name = 'midi-2-keyboard.exe'"` shows whose build it is), wait for it to go away, quit your own
  instance when you are done, and never kill one you did not start.

## Working in parallel (multiple agents)

Several agents may work on this repository at once.

- Work in **your own clone** at `.agents/<your-name>/` inside this repository (git-ignored), not in
  the shared root checkout. Create it once:
  `git clone https://github.com/user1303836/midi-2-keyboard.git .agents/<your-name>`.
  Run every command from inside that clone.
- Stay inside the files/crate you were assigned. If you need a change in someone else's area
  (especially the shared types in `m2k-core/src/model`), make it small and additive, and tell the
  lead agent.
- Integrate continuously: `git pull --rebase origin main`, build, `git push origin HEAD:main`.
  Resolve rebase conflicts carefully; never force-push, never rewrite `main`.
- Stage only your own files (`git add <paths>`), never `git add -A` blindly.
