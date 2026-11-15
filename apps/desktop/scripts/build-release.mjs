#!/usr/bin/env node
// `npm run build`: the release build. On macOS it refuses to make an
// ad-hoc signed app: macOS ties Accessibility, Screen Recording and
// Automation grants to the exact signature of an ad-hoc build, so every
// update would silently lose them while System Settings still shows hame
// switched on, and Gatekeeper refuses such an app on other Macs. Set
// APPLE_SIGNING_IDENTITY to a Developer ID Application identity (and the
// notarization variables, see README.md). For a local build use
// `npm run build:app` (ad hoc) or sign it with a stable identity of your own.
// (src-tauri/build.rs refuses the same for a direct `tauri build`.)
//
// Extra arguments go to `tauri build`, e.g.
// `npm run build -- --features updater --config updater.conf.json`.

import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, statSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(dirname(fileURLToPath(import.meta.url)));
const crate = join(here, "src-tauri");
const mac = process.platform === "darwin";
const args = process.argv.slice(2);

function fail(message) {
  console.error(`build-release: ${message}`);
  process.exit(1);
}

if (mac) {
  const identity = (process.env.APPLE_SIGNING_IDENTITY ?? "").trim();
  if (!identity || identity === "-") {
    fail(
      "a macOS release must be signed with a stable identity; set APPLE_SIGNING_IDENTITY " +
        '(e.g. "Developer ID Application: Name (TEAMID)"). An ad-hoc build loses its ' +
        "Accessibility and Screen Recording grants on every update. For a local build use " +
        "`npm run build:app`.",
    );
  }
}

// A release build must not carry the local builds' opt-out.
const env = { ...process.env };
delete env.HAME_ALLOW_ADHOC;

// Whole seconds: some file systems keep coarse modification times.
const started = Math.floor(Date.now() / 1000) * 1000;
const tauri = join(here, "node_modules", ".bin", process.platform === "win32" ? "tauri.cmd" : "tauri");
const result = spawnSync(tauri, ["build", ...args], {
  cwd: here,
  env,
  stdio: "inherit",
  shell: process.platform === "win32",
});
if (result.status !== 0) process.exit(result.status ?? 1);

if (mac) {
  // Cargo puts the bundles under its target folder (CARGO_TARGET_DIR,
  // relative to the crate, else src-tauri/target), below `release` or
  // `<triple>/release` for --target (universal-apple-darwin too).
  const targetDir = process.env.CARGO_TARGET_DIR
    ? isAbsolute(process.env.CARGO_TARGET_DIR)
      ? process.env.CARGO_TARGET_DIR
      : resolve(crate, process.env.CARGO_TARGET_DIR)
    : join(crate, "target");
  const found = [];
  const look = (dir, depth) => {
    if (depth > 4 || !existsSync(dir)) return;
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const path = join(dir, entry.name);
      if (entry.name.endsWith(".app") && dir.endsWith(join("bundle", "macos"))) {
        if (statSync(path).mtimeMs >= started) found.push(path);
      } else if (!entry.name.endsWith(".app")) {
        look(path, depth + 1);
      }
    }
  };
  look(targetDir, 0);
  if (found.length === 0) fail(`no app bundle built by this run was found under ${targetDir}`);
  for (const app of found) {
    const shown = spawnSync("codesign", ["-dv", app], { encoding: "utf8" });
    const details = `${shown.stdout}${shown.stderr}`;
    if (shown.status !== 0 || /Signature=adhoc/.test(details)) {
      fail(`${app} is not signed with a stable identity:\n${details}`);
    }
    console.log(`build-release: ${app} is signed (${details.match(/Authority=.*/)?.[0] ?? "identity shown above"})`);
  }
}
