import { existsSync, readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vitest/config";
import react from "@vitejs/plugin-react";

// The build goes straight into the Rust crate that embeds it, so `hame web`
// serves exactly what was built here. Output names carry a content hash and
// no source maps are written: the same sources and lockfile give the same
// bytes, which scripts/check-ui-dist.sh relies on. HAME_UI_OUT builds
// somewhere else (the check script builds into a temporary folder).
const root = fileURLToPath(new URL("../..", import.meta.url));
const outDir = join(root, "crates/hame-web/ui-dist");

/** The package an id of a bundled module comes from: its folder, name and package.json. */
function packageOf(id: string): { dir: string; name: string } | undefined {
  const path = id.replace(/^\0/, "").split("?")[0].replace(/\\/g, "/");
  const at = path.lastIndexOf("/node_modules/");
  if (at < 0) return undefined;
  const rest = path.slice(at + "/node_modules/".length).split("/");
  const name = rest[0].startsWith("@") ? `${rest[0]}/${rest[1]}` : rest[0];
  return { dir: `${path.slice(0, at)}/node_modules/${name}`, name };
}

/**
 * Writes `licenses.txt` next to the page: the license text of every npm
 * package with code in the bundle, in name order, so the notices ship with
 * the code they cover. The packages come from Rollup's output (the modules
 * of each chunk, and the stylesheets and assets they pulled in), not from
 * the lockfile, so a package appears exactly when it is built in. Also
 * writes `licenses.json` (name, version, license) for
 * scripts/license-report.py.
 */
function licenses(): Plugin {
  return {
    name: "hame-licenses",
    apply: "build",
    generateBundle(_options, bundle) {
      const ids = new Set<string>();
      for (const file of Object.values(bundle)) {
        if (file.type === "chunk") {
          for (const [id, module] of Object.entries(file.modules)) if (module.renderedLength > 0) ids.add(id);
        }
      }
      // Stylesheets (and what they reference) leave no rendered code in a
      // chunk; the module graph still knows they were built in.
      for (const id of this.getModuleIds()) {
        if (/\.(css|woff2?|ttf|svg|png)(\?|$)/.test(id) && this.getModuleInfo(id)?.isIncluded !== false) ids.add(id);
      }
      const packages = new Map<string, { dir: string; name: string }>();
      for (const id of ids) {
        const pkg = packageOf(id);
        if (pkg && !packages.has(pkg.dir)) packages.set(pkg.dir, pkg);
      }
      const parts = [
        "Open-source software in hame's web interface",
        "",
        "hame itself is MIT licensed. The page includes the packages below; each license",
        "text is copied from the package as published.",
        "",
      ];
      const listed: { name: string; version: string; license: string }[] = [];
      for (const { dir, name } of [...packages.values()].sort((a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0))) {
        const manifest = JSON.parse(readFileSync(join(dir, "package.json"), "utf8")) as { version?: string; license?: string };
        const version = manifest.version ?? "";
        const license = manifest.license ?? "unknown";
        if (listed.some((entry) => entry.name === name && entry.version === version)) continue;
        listed.push({ name, version, license });
        const file = existsSync(dir) ? readdirSync(dir).sort().find((entry) => /^(licen[cs]e|copying)(\.|$)/i.test(entry)) : undefined;
        const text = file ? readFileSync(join(dir, file), "utf8").trim() : `License: ${license} (no license file in the package)`;
        parts.push("=".repeat(72), `${name} ${version} (${license})`, "=".repeat(72), "", text, "");
      }
      this.emitFile({ type: "asset", fileName: "licenses.txt", source: parts.join("\n") });
      this.emitFile({ type: "asset", fileName: "licenses.json", source: `${JSON.stringify(listed, null, 2)}\n` });
    },
  };
}

export default defineConfig({
  plugins: [react(), licenses()],
  base: "/",
  build: {
    outDir: process.env.HAME_UI_OUT ?? outDir,
    emptyOutDir: true,
    sourcemap: false,
    target: "es2022",
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
    reportCompressedSize: false,
  },
  server: {
    // `npm run dev` talks to a running `hame web`: HAME_WEB=http://127.0.0.1:PORT
    proxy: process.env.HAME_WEB ? { "/api": process.env.HAME_WEB } : undefined,
  },
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});
