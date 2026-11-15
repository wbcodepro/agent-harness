# apps/ui — hame's web interface

One interface for the browser (`hame web`), the desktop app and the phone
apps. It follows `docs/design/UI.md` and talks to a hame host through the
`Transport` interface in `src/transport` (see the README there).

## Working on it

From the repository root:

```sh
npm ci                     # once
npm test                   # unit tests (vitest): event folding, diffs, tool descriptions
npm run build              # type-checks and builds into crates/hame-web/ui-dist
sh scripts/check-ui-dist.sh  # fails if ui-dist is not what the sources build
```

`hame web` embeds `crates/hame-web/ui-dist` at compile time (the
`include_dir` crate), so the build output is committed and `cargo install`
needs no Node. After changing anything here, run `npm run build` and commit
`ui-dist` with the change; the check script and the build are byte for byte
reproducible (hashed file names, no source maps, exact dependency versions
from `package-lock.json`).

To develop with hot reload against a running server:

```sh
hame web --port 7700                       # prints http://127.0.0.1:7700/?token=…
HAME_WEB=http://127.0.0.1:7700 npm run dev --workspace apps/ui
# open the Vite address with ?token=… from the line above
```

End-to-end test (Playwright for Python with Google Chrome, scripted model,
no provider is called):

```sh
cargo build -p hame-cli
python3 apps/ui/e2e/test_chat.py           # also refreshes docs/design/screens/*.png
```

## Layout of the code

| Path | What |
|---|---|
| `src/transport/` | `Transport` interface, `local` (HTTP + SSE) and the `remote` placeholder |
| `src/model/types.ts` | wire types: session records and stream events |
| `src/model/fold.ts` | folds the event stream into the view model (work groups, tool rows, approvals, diffs, panel data) |
| `src/model/diff.ts` | line diffs and hunks from edit tool inputs and `apply_patch` patches |
| `src/model/tools.ts` | tool categories, one-line descriptions, work group summaries |
| `src/state/` | API client, per-session store (one repaint per frame), helpers |
| `src/components/` | sidebar, header, transcript, composer, approval card, side panel, Markdown |
| `src/styles/global.css` | the design tokens of UI.md, light and dark |

## Dependencies

Few, all permissively licensed. Exact versions are pinned in
`package.json`; the lockfile pins the rest.

Shipped in the page:

| Package | Version | License | Why |
|---|---|---|---|
| react, react-dom | 19.3.1 | MIT | UI |
| @tanstack/react-virtual | 3.14.24 | MIT | virtualized transcript for long sessions |
| micromark, micromark-extension-gfm | 4.0.3, 3.0.0 | MIT | Markdown (CommonMark + GFM), safe by default |
| highlight.js (core + 14 languages) | 11.12.0 | BSD-3-Clause | code highlighting, loaded on demand as its own chunk |
| lucide-react | 1.64.0 | ISC | icons |

The transitive runtime packages (micromark's parts, `scheduler`, …) are
all MIT.

Build and test only (not shipped):

| Package | Version | License |
|---|---|---|
| vite | 7.3.7 | MIT |
| @vitejs/plugin-react | 5.2.0 | MIT |
| typescript | 5.9.3 | Apache-2.0 |
| vitest | 4.1.12 | MIT |
| @types/react, @types/react-dom | 19.3.0 | MIT |

Their transitive packages are MIT, ISC, Apache-2.0 and BSD-3-Clause, plus
`caniuse-lite` (CC-BY-4.0, browser support data read at build time).

On the Rust side `hame-web` adds `include_dir` 0.7.4 (MIT) to embed the
build.
