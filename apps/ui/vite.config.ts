import { fileURLToPath } from "node:url";
import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// The build goes straight into the Rust crate that embeds it, so `hame web`
// serves exactly what was built here. Output names carry a content hash and
// no source maps are written: the same sources and lockfile give the same
// bytes, which scripts/check-ui-dist.sh relies on. HAME_UI_OUT builds
// somewhere else (the check script builds into a temporary folder).
const outDir = fileURLToPath(new URL("../../crates/hame-web/ui-dist", import.meta.url));

export default defineConfig({
  plugins: [react()],
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
