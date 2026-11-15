# Changelog

All notable changes to hame are listed here. The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Until 1.0, the configuration and session formats may change between versions; such changes are called out.

## Unreleased

### Security

- Browser downloads could write outside the workspace: a destination that was a symbolic link (for example `out.csv -> ~/.zshrc`, created by the sandboxed agent) was followed, a directory path through a link was created outside, and a page's suggested file name could replace any existing file in the destination directory. Downloads now refuse a destination that is a link or leads outside through one, never replace an existing file unless the call passes `overwrite: true` (and then only a regular file), are written to a new temporary file and renamed into place through directories opened without following links, and reduce a suggested name to a plain file name (no separators, `..`, leading dots, control characters or Windows-reserved names). The file stored as an artifact is the one the helper wrote (checked by SHA-256).
- Browser uploads read the file again below the workspace without following links, refuse a file with other hard links, and give the browser a private copy, so a file swapped for a link after the check is not uploaded.
- The browser's redirect guard resolved `Location` with Python's `urljoin`, which keeps `/\evil.example/x`, `\\evil.example/x`, `http:\\evil.example/x` and `http:///evil.example/x` on the granted host while Chrome goes to evil.example. Locations are now resolved as WHATWG (and Chrome) do, an unclear one is refused, and each redirected request is checked again at the URL Chrome computed. `open` and `navigate` hand the helper the WHATWG form of the URL.

- Desktop keys, typed newlines and pixel clicks could commit or save under a grant without `commit` or `file_write` (Return in a save panel, cmd+s, a click on Save). They are now classified from the focused element or the element under the point at dispatch time and need what pressing that control needs.
- `command` acceptance conditions ran unconfined in the harness process. They now run in the sandbox under the agent's policy, with the network blocked and no inherited environment; workflow parameters are shell-quoted into them.
- A custom `HAME_HOME` (other than its `spill` and `worktrees` folders) and a custom `profiles_dir` were readable by shell tools; they are now hidden.
- Commands in the macOS sandbox inherit the terminal's privacy permissions, so they could script applications (`osascript`), read and drive them through accessibility, post keyboard and mouse events, capture the screen and use the clipboard, outside the desktop tool's grants. The Seatbelt profile now refuses Apple Events, the per-app `com.apple.axserver` port, WindowServer and `replayd`, the HID user clients and the pasteboard.
- Return or a typed newline in any text area outside a dialog needed only `global_input`, but in Messages, Slack and other chat apps it sends. It now needs `commit` except in known document editors (TextEdit, Pages, Numbers, Keynote, Notes, Word).
- File acceptance conditions (`file_contains`, `file_sha256`, `json_value`, `csv_shape`) read files unconfined, so a link the agent made to a hidden file (for example `~/.hame/config.toml`) was followed and part of its content returned to the model as evidence. In a confined session they now read through the sandbox.
- `file_absent` passed when the path could not be looked at (a permission error); only "not found" counts as absent now, and a dangling link counts as present.
- `csv_shape` counted lines, so a record with a quoted line break counted twice, and it accepted rows missing a column; it now parses records strictly (unclosed or stray quotes are errors) and fails when a record's width differs from the header.
- `hame run --inbox DIR`: the directory is hidden from the agent's tools (otherwise the agent could write its own "user" messages), and a message file that is a link or has other hard links is not delivered.
- `~/.hame/mcp-auth` (MCP OAuth tokens) is hidden from shell tools even when it is created after the session starts.
- A browser tab whose redirect guard could not be attached was used unguarded; it is now refused until the guard attaches.
- More control names count as commits on the desktop: post, reply, confirm, order, remove, merge.
- A password typed into a secure field was recorded in the action receipt; it is now left out.
- A browser `fill` of a password field echoed the field's value in the result and recorded it as receipt evidence (`value=...`); both are now left out.
- Unknown fields in task files, grants, budgets and predicates, and predicates with empty fields, are errors instead of being ignored or passing vacuously; a required browser profile that is not reported fails the check.

### Added

- Grants take `folders`: a confirmed desktop save or export panel may write only into them (default the workspace; `"*"` for any folder). Before, `file_write` on an app let it save anywhere (a pilot run exported into `~/Documents`). The desktop helper protocol is now 3.

- Desktop: `select` (table cells by address, text by occurrence), cell addresses and the Numbers and Pages canvases in observations, typing that follows Numbers' formula editor; an Export button in a save panel needs `commit` and `file_write` like Save.
- `hame run --inbox DIR` and the eval manifest field `interject`: a user message delivered mid-run.
- `[computer] observation = "pixels"` (opt-in, needs a `screen` grant): the screenshot-loop baseline for measuring what the default semantic observations save. The browser tool then shows every page as a viewport screenshot with a one-line header and no page text or refs, and offers only `open`, `navigate`, `back`, `tabs`, `wait` (time or load state), `dialog`, `close`, `screenshot` and the coordinate actions `click_at` (with optional `upload` or `download`), `type`, `press` and `scroll`, each returning a fresh screenshot; `batch` is never offered. A click by coordinates is authorized like a click by ref: the element under the point is resolved before dispatch and checked again just before the click, a form submission (or Enter in a form field, or a typed newline) needs `commit` and is journalled with an idempotency key, links to ungranted origins are refused, a click into an embedded frame counts as a commit, and text typed into a password field is not recorded. Unknown `observation` values are now a configuration error.
- With `observation = "pixels"` the desktop tool also works from screenshots only: `apps`, `windows`, `focus` (a window), `screenshot`, `click_at`, `type`, `key`, `menu` and `wait` (window, dialog or title); no observations, refs or batches; every acting call is answered with a screenshot of its window. Its pixel clicks need a grant on the `visual` channel. `menu` stays because window screenshots do not show the menu bar.
- Desktop `save`: writes a window's document through the app's scripting interface (Apple Events), so it works with the app in the background: in place, to an absolute path, or exported (`format`: Numbers pdf/xlsx/csv, Pages pdf/docx) for TextEdit, Numbers and Pages. Needs `commit` and `file_write`; the destination is checked by its real path against the grant's `folders` (a folder or its direct subfolder), and an existing file is replaced only with `overwrite`. macOS asks the user once to allow controlling each app (Automation).
- `[computer] desktop_background = true`: the desktop tool works without taking the user's screen. Text and plain keys go to the target app's process without bringing it in front (the target window must be focused inside its app); `focus`, `menu`, `click_at` and keys with cmd, ctrl or opt are refused as `needs_foreground`, because an app in the background does not carry out menu commands or key equivalents (measured on macOS 26.6.2; `docs/research/BACKGROUND-EXECUTION-2026-11-15.md`). `select` takes `caret: "before"` or `"after"` to place an insertion point instead of selecting.
- Desktop window lines end with `[file: path]` when the application reports the file the window shows (AXDocument), so a save can be checked by folder, name and extension; window titles hide extensions.
- `evals/netem_proxy.py` (a CONNECT proxy that adds round-trip time and limits upload bandwidth for model traffic), `evals/pilot/arms/` (the screenshot-loop, one-action and default configurations) and `evals/pilot/compare.py` (reductions against a baseline arm with task-resampled bootstrap intervals). `hame eval --filter` takes comma-separated id parts.

### Changed

- The Anthropic adapter sends an error tool result that carries an image (a screenshot after a refused action) without `is_error`, which the API allows only for all-text content; its content starts with `[tool error]`.
- A task's `max_tokens` caps the token budget it shares with subagents; a task with `max_cost_usd` gets no subagent tool.
- A Control-effect call falls back to Allow only when the configured default is Ask; a Deny default applies to it.
- Browser `download` and `wait` (with `download`) take `overwrite`; without it an existing file is refused as `[exists]`. A workflow that downloads to the same path on every run needs `overwrite = true` in that step.

## 0.1.0-rc.1 — 2026-11-15

Release candidate. Details, breaking changes and verification: [docs/release/RELEASE-NOTES.md](docs/release/RELEASE-NOTES.md).

### Breaking

- Session logs are format version 2 (typed image, artifact, data and unsupported blocks; `task/start`, `action/receipt` and `task/verified` events). Version 1 logs still open; 0.0.1 refuses version 2 logs.
- Embedding API: `ToolOutput.parts`, `ToolContext.artifacts`/`journal`/`session_id`, `Effect::Control`, new `Block` and `Event` variants, `Completion.meta` and `Request.artifacts`.

### Added

- `hame task FILE`: a TOML task file (outcome, constraints, acceptance conditions, grants, budget); acceptance conditions are checked against the real state (files, SHA-256, JSON pointer, CSV shape, command exit, live browser page, live application window); failing conditions send the agent back at most twice; exit 0 only when verified; `--json` and `--trace`.
- Token, step and time budgets enforced before every model request of a task.
- `browser` tool: Chrome (or Edge, or Playwright's Chromium) through a persistent Playwright helper; observation-scoped refs, `stale_ref`/`ambiguous` refusals, read-back fills, profiles, tabs, waits, extraction, uploads, downloads, dialogs, screenshots, batches.
- `desktop` tool (macOS 14+): a Swift accessibility helper compiled on first use; observe, press, set value, focus, menus, chunked Unicode typing with interference detection, keys, window screenshots and pixel clicks with recorded transforms, clipboard with restore, waits, batches; a cross-process lease for physical input.
- `[computer]` configuration and task-file grants: channel × app/origin/profile × capability, checked before every action; observed content cannot grant.
- Action journal with receipts and idempotency keys; crash reconciliation marks dispatched actions `outcome_unknown` and never replays them.
- Content-addressed artifact store (`~/.hame/artifacts`) for images and files from tools and MCP servers; `hame sessions gc`; resume warns about missing artifacts; forks keep receipts.
- Native images and PDFs for Anthropic, OpenAI Responses and chat completions; model profiles with `[model] vision` and `pdf` overrides; explicit notes for images a model cannot take.
- MCP results keep images, resources, links and structured content.
- Images shown in the web interface and ACP clients, described in the terminal interface.
- Workflows (`.hame/workflows/*.toml`) with parameters, required grants, pre- and postconditions, trusted per file hash; `hame workflow list | show | trust | untrust`.
- `--trace` and `HAME_TRACE`: `trace.v1.jsonl` with spans and per-attempt usage; `hame eval` timing split, outcomes including `false_success`, dated tariff costs, `--arm`/`--set` comparisons with bootstrap intervals; `evals/pilot`.
- `hame doctor` checks that the artifact store is writable, the browser tool's Python, Playwright and browser, and (macOS) builds the desktop helper and reports its Accessibility and Screen Recording permissions.
- Release tooling: `scripts/docker-clean-install.sh`, `scripts/license-report.py` (third-party license report and CycloneDX SBOM), `scripts/release-check.sh`; `docs/release/` install guide, support matrix, release notes and instructions.

### Changed

- Old images leave the prompt three at a time (at least the newest three stay), so the cached prefix survives between steps.
- The action journal syncs to disk only on `dispatched`.
- Workspace version 0.1.0-rc.1.

### Fixed

- Inner calls of the `script` tool to tools that are not host-side run in the sandbox; they ran in the harness process before.

### Also since 0.0.1 (onboarding and reliability)

Added:

- `hame setup`: choose a provider, save its key to `~/.hame/.env` (hidden input, owner-only file), and check that everything works.
- `hame doctor [--ping]`: check configuration, model and key, sandbox, `bash`, `git` and `python`, and optionally send one test request.
- A `bash` command still running at its time limit becomes a background job in the sandbox instead of being killed; the new `job_output` tool waits for it, reads it or stops it. A check still running is not counted as passing.
- `Retry-After` (and `retry-after-ms`) is honoured up to 60 seconds; longer waits are reported at once. Plain backoff has jitter.
- A request refused because the prompt exceeds the context window is compacted and sent again once.
- When the summary model is the main model, compaction reuses the conversation's cached prefix.
- English README, `LICENSE`, `SECURITY.md`, `CONTRIBUTING.md` and issue templates.

Changed:

- The message for a missing API key names every provider's key, where to put it, and `hame setup`.

## 0.0.1

First version, never released as a package. Highlights:

- One Rust binary with a terminal interface, `hame run`, a web interface and an Agent Client Protocol server (tested in Zed and IntelliJ IDEA).
- Providers: DeepSeek, Anthropic, OpenAI (Responses and chat completions) and compatible servers.
- Tools confined by the operating system (Seatbelt, Landlock and seccomp, Low integrity on Windows), network allowlist through a local proxy, secret substitution at the proxy.
- Completion checks: project check commands, `stop_on_red`, checklist, optional independent reviewer; model escalation when stuck.
- Append-only session log with resume, fork, rewind and workspace checkpoints.
- MCP (stdio, HTTP, stateless, OAuth), skills, Claude Code hooks and plugins, WebAssembly plugins.
- `hame eval` and `hame probe` with task suites that have hidden checks.
