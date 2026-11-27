// Runs a command with the project's portable build tools on PATH.
//
// On Windows with the Rust GNU toolchain, building needs `dlltool` and `windres` from MinGW-w64.
// If a portable MinGW lives in `.tools/mingw64` (in this directory or any parent, so per-agent
// clones under `.agents/<name>/` find the shared copy), its `bin` directory is prepended to PATH.
// Release builds additionally get their home-directory paths rewritten (see below). Everything
// else is a transparent pass-through.
//
//   node scripts/with-tools.mjs cargo check --workspace
//   node scripts/with-tools.mjs tauri dev

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import process from "node:process";

const [command, ...args] = process.argv.slice(2);
if (!command) {
  console.error("usage: node scripts/with-tools.mjs <command> [args...]");
  process.exit(2);
}

const env = { ...process.env };

if (process.platform === "win32") {
  let dir = process.cwd();
  for (;;) {
    const bin = path.join(dir, ".tools", "mingw64", "bin");
    if (existsSync(bin)) {
      const key = Object.keys(env).find((k) => k.toLowerCase() === "path") ?? "Path";
      env[key] = `${bin}${path.delimiter}${env[key] ?? ""}`;
      break;
    }
    const parent = path.dirname(dir);
    if (parent === dir) break;
    dir = parent;
  }
}

// Release builds do not carry the home directory of whoever built them. Rust keeps the source
// path of every dependency for its panic messages (…/.cargo/registry/src/…), a few hundred of
// them, so an exe or installer built on someone's machine would otherwise name their account.
// The paths are rewritten to start at "~". Only for release builds (the debug caches stay
// valid), and only when the environment sets no rustflags of its own — which would replace any
// rustflags from a cargo config file, as these do; M2K_KEEP_PATHS=1 switches this off.
const release =
  (command === "tauri" && args[0] === "build" && !args.includes("--debug")) ||
  (command === "cargo" && (args.includes("--release") || args.includes("-r")));
if (release && !env.RUSTFLAGS && !env.CARGO_ENCODED_RUSTFLAGS && !env.M2K_KEEP_PATHS) {
  const home = os.homedir();
  const remap = [
    [env.CARGO_HOME ?? path.join(home, ".cargo"), "~/.cargo"],
    [env.RUSTUP_HOME ?? path.join(home, ".rustup"), "~/.rustup"],
    [home, "~"],
  ];
  env.CARGO_ENCODED_RUSTFLAGS = remap.flatMap(([from, to]) => ["--remap-path-prefix", `${from}=${to}`]).join("\x1f");
}

// Prefer locally installed CLIs (node_modules/.bin), e.g. `tauri`.
const localBin = path.join(process.cwd(), "node_modules", ".bin");
if (existsSync(localBin)) {
  const key = Object.keys(env).find((k) => k.toLowerCase() === "path") ?? "PATH";
  env[key] = `${localBin}${path.delimiter}${env[key] ?? ""}`;
}

// On Windows the CLIs in node_modules/.bin are .cmd shims, which need a shell. Build the command
// line ourselves (quoting what needs it) instead of handing an args array to a shell.
const quote = (value) => (/[\s"&|<>^()]/.test(value) ? `"${value.replace(/"/g, '\\"')}"` : value);
const child =
  process.platform === "win32"
    ? spawn([command, ...args].map(quote).join(" "), { stdio: "inherit", env, shell: true })
    : spawn(command, args, { stdio: "inherit", env });
child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exit(code ?? 1);
});
child.on("error", (error) => {
  console.error(`failed to run ${command}: ${error.message}`);
  process.exit(1);
});
