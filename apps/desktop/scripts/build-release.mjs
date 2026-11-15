#!/usr/bin/env node
// `npm run build`: the release build. On macOS it refuses to make an
// ad-hoc signed app: macOS ties Accessibility, Screen Recording and
// Automation grants to the exact signature of an ad-hoc build, so every
// update would silently lose them while System Settings still shows hame
// switched on, and Gatekeeper refuses such an app on other Macs. Set
// APPLE_SIGNING_IDENTITY to a Developer ID Application identity (and the
// notarization variables, see README.md). For a local build use
// `npm run build:app` (ad hoc) or sign it with a stable identity of your own.
//
// Extra arguments go to `tauri build`.

import { spawnSync } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(dirname(fileURLToPath(import.meta.url)));
const mac = process.platform === "darwin";

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

const tauri = join(here, "node_modules", ".bin", process.platform === "win32" ? "tauri.cmd" : "tauri");
const result = spawnSync(tauri, ["build", ...process.argv.slice(2)], {
  cwd: here,
  stdio: "inherit",
  shell: process.platform === "win32",
});
if (result.status !== 0) process.exit(result.status ?? 1);

if (mac) {
  // The configuration could still have produced an ad-hoc bundle (a
  // signingIdentity of "-" merged in); check what was built.
  const bundles = join(here, "src-tauri", "target", "release", "bundle", "macos");
  const apps = existsSync(bundles) ? readdirSync(bundles).filter((name) => name.endsWith(".app")) : [];
  for (const app of apps) {
    const shown = spawnSync("codesign", ["-dv", join(bundles, app)], { encoding: "utf8" });
    const details = `${shown.stdout}${shown.stderr}`;
    if (shown.status !== 0 || /Signature=adhoc/.test(details)) {
      fail(`${app} is not signed with a stable identity:\n${details}`);
    }
  }
}
