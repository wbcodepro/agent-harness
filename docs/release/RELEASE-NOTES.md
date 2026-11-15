# hame 0.1.0-rc.1 — release notes

Release candidate. hame was a coding agent; this version lets it carry out tasks whose outcome it checks itself, in files, a web browser and native macOS applications, and keeps the evidence. Build from source ([INSTALL.md](INSTALL.md)); see [SUPPORT-MATRIX.md](SUPPORT-MATRIX.md) for what was tested where and [EVIDENCE.md](EVIDENCE.md) for measured results.

## What's new

**Tasks with a verified outcome.** `hame task FILE` reads a TOML task file (outcome, constraints, acceptance conditions, grants, budget), works until the conditions hold, then checks them against the real state: files, SHA-256, JSON pointers, CSV shape, command exit codes, the live browser page, a native application's window. Failing conditions send the agent back (at most twice). It exits 0 only when verified, and `--json` prints the verdict with per-condition evidence, action receipts, tokens and timings. Token, step and time budgets stop the task before the next model request.

**Browser tool.** A persistent Playwright helper drives Google Chrome (or Edge, or Playwright's Chromium) with semantic observations: roles, names, values, refs scoped to one observation and document. Stale or changed elements are refused (`stale_ref`), never re-targeted; duplicate labels are disambiguated or refused as `ambiguous`; fills are read back. Persistent profiles, tabs, waits, extraction with pagination, uploads from and downloads into the workspace, dialogs, screenshots and batches of up to 20 steps.

**Desktop tool (macOS 14+).** A small Swift helper, compiled on first use, operates applications through the accessibility interface: apps, windows, observe, press, set value, focus, menus, typing (Unicode, chunked, stops on focus changes or unexpected dialogs), key combinations, window screenshots with recorded transforms and clicks mapped back from screenshot pixels, clipboard with restore, waits and batches.

**Grants.** `[computer]` in `config.toml` and `[[authorization.grants]]` in task files authorize channel (browser, desktop, visual) × app / origin / profile × capability (observe, interact, commit, file_write, clipboard, global_input, screen). Every action is checked before dispatch; observed content is labelled untrusted and can never add a grant. On the desktop, keys, typed newlines and pixel clicks that would press a commit control or confirm a save panel need `commit` (and `file_write`) like pressing that control does, classified from the focus or the element under the point at dispatch time.

**Action journal and receipts.** Every acting operation is journalled before dispatch and settled after; commits carry idempotency keys and are not repeated while an earlier attempt may have taken effect. After a crash, dispatched actions become `outcome_unknown` and the agent is told to check before retrying; nothing is replayed.

**Images end to end.** Session format 2 has typed image, artifact, data and unsupported blocks. Images and files are kept once in a content-addressed store (`~/.hame/artifacts`, SHA-256, fsynced before the log refers to them). Native image transport for Anthropic (image and document blocks, also inside tool results), OpenAI Responses (`input_image`, `input_file`) and chat completions, with model profiles (vision, PDF, limits) and `[model] vision` / `pdf` overrides; images a model cannot take become explicit notes. MCP results keep images, resources, links and structured content. The web interface and ACP clients display images; the terminal interface describes them. Old images leave the prompt three at a time so the cached prefix survives.

**Workflows.** Saved, parameterized tool sequences (`.hame/workflows/*.toml`, `~/.hame/workflows/`) with preconditions (drift stops them before any step), postconditions, parameter patterns and required grants. They run only when the user trusted that exact file: `hame workflow list | show | trust | untrust`.

**Traces and evaluation.** `--trace` writes `trace.v1.jsonl` (spans for turns, model attempts including retries, tool calls including inner ones, verification; normalized and raw usage). `hame eval` records setup, agent, verification and total time, outcomes including `false_success`, costs from a dated tariff (unknown stays unknown), and compares configurations with `--arm`/`--set`, interleaved, with bootstrap confidence intervals. A first pilot task set is in `evals/pilot`.

**Storage.** `hame sessions gc [--older-than-hours N] [--dry-run]` removes artifacts no session refers to. Resuming warns about missing or damaged artifacts. Forks carry settled action receipts.

**Onboarding.** `hame setup` and `hame doctor`; doctor now also checks that the artifact store is writable, the browser tool's Python, Playwright and browser, and on macOS builds and starts the desktop helper to report the Accessibility and Screen Recording permissions it will run with.

**Security fix.** Inner calls of the `script` tool to tools that are not host-side now go through the sandboxed runner; before, they ran in the harness process.

## Breaking changes

- **Session format 2.** Logs written by 0.1.0-rc.1 have `version: 2` in their header. Builds before it (0.0.1) refuse them: "session format version 2 is newer than this build supports (1)". Version 1 logs open unchanged in this version. Copying a session to another machine now also needs `~/.hame/artifacts`.
- **Embedding API (Rust crates).** For code that uses hame's crates directly:
  - `hame_tools::ToolOutput` has a new field `parts: Vec<Block>` (typed content beside `text`); use `ToolOutput::content()` to get both. Struct literals must add `parts` (or use the constructors).
  - `hame_tools::ToolContext` has `artifacts`, `journal` and `session_id` (builder methods `with_artifacts`, `with_journal`, `with_session`).
  - `hame_tools::Effect` has a new variant `Control` (acts on applications outside the sandbox); exhaustive matches need an arm.
  - `hame_session::Block` has new variants `Image`, `Artifact`, `Data`, `Unsupported`; `Event` has `TaskStart`, `ActionReceipt`, `TaskVerified`. Use `Block::as_text` / `blocks_text` for a text view.
  - `hame_llm::Completion` has a new field `meta: CallMeta` (model, request and response bytes, images, time to first token, raw usage); `hame_llm::Request` has `artifacts: Option<ArtifactSource>`. Adapters have `with_profile(ModelProfile)`.
  - The sandbox worker protocol carries `parts`; a worker from an older build cannot serve this one (they ship in the same binary, so this only matters for custom setups).
- **Configuration.** New optional sections and keys only (`[computer]`, `[model] vision`, `[model] pdf`); existing files load unchanged. Unknown keys are still errors, so a config using `[computer]` does not load in 0.0.1.
- **Version.** The workspace version moved from 0.0.1 to 0.1.0-rc.1; `hame --version` reports it.

## Dependencies

- No new third-party Rust crates: the new crates (`hame-browser`, `hame-desktop`, `hame-trace`) use crates already in the lockfile. 432 third-party crates are built for the shipped platforms; all are permissive except `im-rc`, `bitmaps` and `sized-chunks` (MPL-2.0, used unmodified through wasmtime) and `terminfo` (WTFPL). See [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) and the CycloneDX SBOM [sbom.cdx.json](sbom.cdx.json).
- Optional runtime components, installed by the user and not distributed: Python 3 with Playwright for Python (Apache-2.0) and Google Chrome for the browser tool; the Swift toolchain (Xcode command line tools) for the desktop tool.

## Known limitations

- Desktop control is macOS only and was tested with TextEdit on macOS 26.6.2; no scroll, drag, hover or text-range selection; US key layout for `key`.
- The browser tool was verified on macOS arm64 with Chrome 157 only; only top-level navigation is limited to granted origins; commit detection is heuristic.
- Browser and desktop tools act outside the shell sandbox with the user's permissions; grant narrowly.
- Prices exist only for Anthropic models; a `max_cost_usd` budget stops a task on other providers because its spending cannot be measured.
- OpenAI image and PDF transport, and Anthropic PDFs, are tested against fixtures, not live.
- Not verified for this release candidate: Linux x86_64, macOS x86_64, Windows (an earlier revision passed on Windows 11).
- No prebuilt binaries; no security audit.

## Verification for this release candidate

- `cargo test --workspace` on macOS 26.6.2 arm64 (Apple M5, rustc 1.99.1, Playwright and Chrome present, so the browser tests ran): 340 passed, 0 failed, 16 ignored (live-provider and desktop GUI tests, which are opt-in). The desktop GUI suite passed 11/11 on the same machine before integration ([EVIDENCE-desktop.md](EVIDENCE-desktop.md)).
- Clean build and `cargo test --workspace` in a fresh Linux arm64 container from `git archive`: 339 passed, 0 failed, 5 ignored ([INSTALL.md](INSTALL.md)).
- `scripts/release-check.sh`: source and macOS arm64 binary archives contain no `.env`, keys, session logs, `target/`, `.claude/` or `.solutions/` ([RELEASE-INSTRUCTIONS.md](RELEASE-INSTRUCTIONS.md)).
