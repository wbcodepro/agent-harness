# Unified harness release tracker

Durable state for implementing [the roadmap](../research/UNIFIED-HARNESS-ROADMAP-2026-11-15.md) to a release candidate. Updated at every integration. Decisions are in [DECISIONS.md](DECISIONS.md).

Branch: `unified-harness` (base commit: shared contracts). Baseline before any change: revision `cc2ac82`, `cargo test --workspace` 233 passed, 0 failed, 1 ignored (macOS 26.6.2, Apple M5, rustc 1.99.1).

## Environment facts (verified 2026-11-15)

- macOS 26.6.2 arm64; Accessibility (`AXIsProcessTrusted`) and Screen Recording (`CGPreflightScreenCaptureAccess`) are granted to the terminal process tree.
- Swift 6.4.2 toolchain present; Python 3.14.5 with `playwright` installed; Google Chrome installed (`/Applications/Google Chrome.app`); Node 24.
- `agent-browser` and `agent-desktop` are not installed.
- Model key: `.env` defines `CLAUDE_API_KEY` (read by `hame-app` as an Anthropic key). Never printed.

## Shared contracts (done in base commit)

| Contract | Location |
|---|---|
| Typed content blocks `Image`, `Artifact`, `Data`, `Unsupported`; `Block::as_text`, `blocks_text` | `crates/hame-session/src/model.rs` |
| Session format version 2; v1 logs read unchanged; v1 builds refuse v2 logs | `model.rs` `FORMAT_VERSION` |
| Events `task/start`, `action/receipt`, `task/verified` | `model.rs` |
| Content-addressed `ArtifactStore` (atomic fsynced write, hash verify, MIME sniffing, dimensions), `ArtifactRef`, `Capture` with crop→screen transform, `ArtifactSource` | `crates/hame-session/src/artifact.rs` |
| `TaskContract`, `Predicate`, `VerificationReport`, `Target`, `Grant`/`Authorization`, `Capability`, `Channel`, `Observation`, `ElementRef`, `Provenance`, `ActionReceipt`/`ActionStatus`, `AdapterManifest`, `NestedUsage`, `ActionJournal` | `crates/hame-session/src/contract.rs` |
| `SessionStore::artifacts()` (`<home>/artifacts`), `SessionStore::journal_path(id)` | `crates/hame-session/src/store.rs` |
| `ToolOutput.parts`, `ToolOutput::content()`; `ToolContext.artifacts/journal/session_id` | `crates/hame-tools/src/lib.rs` |
| Worker protocol carries `parts` | `crates/hame-sandbox/src/worker.rs` |
| `Request.artifacts: Option<ArtifactSource>`; agent sets it from `tool_context.artifacts` | `crates/hame-llm/src/lib.rs`, `crates/hame-agent/src/lib.rs` |

## Workstreams

| ID | Scope | Owned files | Depends on | Status |
|---|---|---|---|---|
| A | Session/artifact runtime: wiring store+journal into sessions, resume detection of missing artifacts, recovery with receipts, GC/retention, context projection & compaction with images, fork/import/export, frontend rendering | `hame-session` (store, projection, recover), `hame-context`, `hame-app/src/import.rs`, `hame-tui`, `hame-web`, `hame-acp`, `hame-cli/src/main.rs` rendering | base | wave 1 |
| B | Providers and MCP: native image serialization (Anthropic, Responses, Chat Completions), capability profiles, raw+normalized usage, request bytes, MCP mixed content | `hame-llm/*`, `hame-ext/src/mcp.rs` | base | wave 1 |
| C | Browser execution: persistent Playwright helper, semantic observations, generation-scoped refs, actionability, waits, uploads/downloads, profiles, receipts, fixture app | new `crates/hame-browser` | base | wave 1 |
| D | Desktop execution (macOS): Swift AX helper, broker/lease, window identity, AX actions, Unicode input, selective screenshots with transforms, interference detection | new `crates/hame-desktop` | base | wave 1 |
| E | Telemetry and evaluation: trace stream with spans, normalized accounting incl. nested/retries, eval timing incl. verification, task manifest, ablation runner, reports | new `crates/hame-trace`, `hame-eval`, `hame-cli/src/eval.rs` | base | wave 1 |
| G | Outcome verification and authorization: predicate checker, `task` contract CLI, computer grant policy layer, workflows with pre/postconditions, app assembly of browser/desktop tools | `hame-verify`, `hame-policy`, `hame-app/src/lib.rs`, `hame-ext/src/script.rs` | A–E | wave 2 |
| F | Independent verification and release: cross-component and fault tests, docs, packaging, licenses/SBOM, clean install, review | docs, `evals/`, CI | all | wave 3 |

## Acceptance criteria status

### G (integrator, on `unified-harness`)

- [x] Outcome predicates checked against real state: files, JSON pointer, CSV shape, command exit, browser/desktop via `StateProbe`; no predicates ⇒ not verified; missing probe ⇒ fail with reason (`hame-verify/src/outcome.rs`).
- [x] `OutcomeGate` turn hook sends failing acceptance checks back to the model (bounded rounds).
- [x] Reusable workflows: parameter schema with patterns, required grants, preconditions (drift ⇒ nothing runs), steps through the tool pipeline and sandbox runner, postconditions, trust list keyed by file SHA-256 (editing revokes trust) (`hame-verify/src/workflow.rs`).
- [x] `[computer]` config with scoped grants → `Authorization`; Control-effect tools fall back to Allow because adapters enforce grants; explicit rules can still ask/deny (`hame-policy`).
- [x] Security fix: `script` inner calls to non-host-side tools now go through the sandboxed runner (previously ran in the harness process).
- [x] Budget guard (tokens, deadline) before every model request (`hame-verify/src/budget.rs`); cost budget waits for the tariff table (E).
- [x] Task files (TOML) → `TaskContract` + task prompt (`hame-verify/src/task_file.rs`).
- [ ] `hame task` CLI + app assembly (browser/desktop tools, OutcomeGate, BudgetGuard, trace wiring, profile overrides) — after wave 1 merges.
- [ ] Browser/desktop/cross-app pilot tasks in the eval manifest; real-API end-to-end runs.

## Test log

| Date | Command | Result |
|---|---|---|
| 2026-11-15 | `cargo test --workspace` at `cc2ac82` | 233 passed, 0 failed, 1 ignored |
| 2026-11-15 | `cargo test --workspace` after shared contracts | 237 passed, 0 failed, 1 ignored; clippy clean |
| 2026-11-15 | `cargo test --workspace` after verification/workflows/grants (1ac63c4) | 243 passed, 0 failed, 1 ignored |
| 2026-11-15 | `cargo test -p hame-verify` after budget + task files (a1d9753) | 20 passed |

## Dependency licenses (pre-merge snapshot, `cargo metadata`, all targets)

514 third-party crates. Permissive (MIT/Apache/BSD/ISC/Zlib/Unicode/CC0/Unlicense) except: `im-rc`, `bitmaps`, `sized-chunks` (MPL-2.0, file-level copyleft; unmodified), `terminfo` (WTFPL), `webpki-root-certs` (CDLA-Permissive-2.0). Dual `GPL-2.0-only OR BSD-3-Clause` is used under BSD-3-Clause. Final report after wave 1 merges.

## Release blockers

None recorded yet.
