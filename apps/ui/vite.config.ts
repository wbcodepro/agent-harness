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

/**
 * Writes `licenses.txt` next to the page: the license text of every npm
 * package built into it (the non-dev packages of package-lock.json), in
 * name order, so the notices ship with the code they cover.
 */
function licenses(): Plugin {
  return {
    name: "hame-licenses",
    apply: "build",
    generateBundle() {
      const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8")) as {
        packages: Record<string, { version?: string; license?: string; dev?: boolean; link?: boolean }>;
      };
      const parts = [
        "Open-source software in hame's web interface",
        "",
        "hame itself is MIT licensed. The page includes the packages below; each license",
        "text is copied from the package as published.",
        "",
      ];
      const names = Object.keys(lock.packages)
        .filter((key) => key.startsWith("node_modules/") && !lock.packages[key].dev && !lock.packages[key].link)
        .sort();
      for (const key of names) {
        const pkg = lock.packages[key];
        const name = key.slice(key.lastIndexOf("node_modules/") + "node_modules/".length);
        const dir = join(root, key);
        const file = existsSync(dir) ? readdirSync(dir).sort().find((entry) => /^(licen[cs]e|copying)(\.|$)/i.test(entry)) : undefined;
        const text = file ? readFileSync(join(dir, file), "utf8").trim() : `License: ${pkg.license ?? "unknown"} (no license file in the package)`;
        parts.push("=".repeat(72), `${name} ${pkg.version ?? ""} (${pkg.license ?? "unknown"})`, "=".repeat(72), "", text, "");
      }
      this.emitFile({ type: "asset", fileName: "licenses.txt", source: parts.join("\n") });
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
