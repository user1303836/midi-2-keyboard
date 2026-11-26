// Runs a command with the project's portable build tools on PATH.
//
// On Windows with the Rust GNU toolchain, building needs `dlltool` and `windres` from MinGW-w64.
// If a portable MinGW lives in `.tools/mingw64` (in this directory or any parent, so per-agent
// clones under `.agents/<name>/` find the shared copy), its `bin` directory is prepended to PATH.
// Everywhere else this is a transparent pass-through.
//
//   node scripts/with-tools.mjs cargo check --workspace
//   node scripts/with-tools.mjs tauri dev

import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
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

// Prefer locally installed CLIs (node_modules/.bin), e.g. `tauri`.
const localBin = path.join(process.cwd(), "node_modules", ".bin");
if (existsSync(localBin)) {
  const key = Object.keys(env).find((k) => k.toLowerCase() === "path") ?? "PATH";
  env[key] = `${localBin}${path.delimiter}${env[key] ?? ""}`;
}

const child = spawn(command, args, { stdio: "inherit", env, shell: process.platform === "win32" });
child.on("exit", (code, signal) => {
  if (signal) process.kill(process.pid, signal);
  else process.exit(code ?? 1);
});
child.on("error", (error) => {
  console.error(`failed to run ${command}: ${error.message}`);
  process.exit(1);
});
