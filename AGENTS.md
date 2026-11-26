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
                device I/O (midir) with hot-plug handling.
  m2k-output/   Key model + OS input injection (keyboard, mouse, unicode text) per platform,
                held-key bookkeeping, platform probes (permissions, foreground app).
  m2k-core/     Config model (the JSON schema), persistence, and the engine: matching,
                behaviors (trigger/hold/toggle/tap-hold/multi-tap/step/...), rate limiting,
                scheduling, conditions, profiles, monitor events.
src-tauri/      Tauri v2 desktop shell: commands/events between the engine and the UI, tray,
                autostart, single-instance.
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
    rename or remove without a migration in `m2k-core`.
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
