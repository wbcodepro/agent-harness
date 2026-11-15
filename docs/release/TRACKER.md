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
- [x] Budget guard (tokens, deadline, cost) before every model request (`hame-verify/src/budget.rs`); cost uses E's dated tariff and stops when a price is unknown (7bafd62).
- [x] Task files (TOML) → `TaskContract` + task prompt (`hame-verify/src/task_file.rs`).
- [x] `hame task` CLI + app assembly: `Computer` (browser/desktop from config + task grants, batch via pipeline, `ComputerProbe`), workflows offered when present, OutcomeGate for tasks, BudgetGuard, trace spans for turn and verification; plan mode drops Control tools (74efb91).
- [x] `[model] vision`/`pdf` overrides wired; `[computer] browser_channel`.
- [x] Browser `page_state` and desktop `app_state` for independent verification.
- [x] Journal fsync only on `dispatched` (D measured 15–21 ms/action of syncs).
- [x] Live: `hame task` Vietnamese file task, claude-haiku-5-5: verified, 3 requests, 4.6 s.
- [x] Pilot browser/desktop/cross-app/recovery tasks + injection suite + ablations with real API (G2, merged): see the G2 section.
- [ ] Remaining 11 tasks of the roadmap's 30-task manifest (D02, D05, O01–O05, X02–X05): agent G3.
- [x] Docs, packaging, clean install (macOS + Linux Docker), licenses/SBOM, release notes (F, merged): see the F section.
- [ ] Independent review (wave 3).

### C — merged (5930db4)
- [x] `hame-browser`: persistent Playwright helper (Chrome channel), profiles, one `browser` tool with observe/act/wait/extract/screenshot/download/upload/dialog/tabs/batch; generation-scoped refs, stale/ambiguous/not_actionable errors; grants per capability; journalled receipts with idempotency keys; batch through pipeline.
- [x] Fixture app + 13 browser tests (B01–B05, stale ref, hidden controls, dialog stops batch, crash-after-submit reconciliation, injection, Vietnamese, commit/screen grants, pipeline).
- [x] ADR-4: direct Playwright chosen over agent-browser 0.41.0 (faster clicks but silent wrong fills and stale-ref retargeting). Evidence in EVIDENCE-browser.md.
- Limits: verified on macOS arm64 only; JS-only posting buttons count as Interact unless `commit: true`; iframes/closed shadow roots not expanded.

### D — merged (c070270, 4907090)
- [x] `hame-desktop`: Swift AX helper compiled on first use; one `desktop` tool (apps, windows, observe, press, set_value, focus, menu, type, key, screenshot, click_at, clipboard, wait, batch); lease for physical input; interference/unexpected-dialog stops; cancellation; receipts.
- [x] 21 unit tests + 11 GUI tests on TextEdit (D01, D04, stale window/ref, ambiguity, interference, cancel, unexpected sheet, screenshot+click_at transforms, clipboard, denied permission, latency).
- [x] ADR-5: own helper chosen (agent-desktop 16× slower observe; HCU needs an extra privacy grant, not run).
- Limits: macOS 14+; no scroll/drag/hover/text-range; US key layout; `wait` polls; only TextEdit tested.

### E — merged (c0751d8)
- [x] `hame-trace`: trace.v1.jsonl spans (all kinds), usage records (normalized + raw), TracedLlm (one span per attempt incl. retries), trace_layer, nested usage, dated Anthropic tariff, cost None when unknown.
- [x] `hame eval`: setup/agent/verification/total timing, outcomes incl. timeout/false_success, §7 records, arms with interleaving and bootstrap CIs, forbidden-effect hashing; `evals/pilot` F01–F05 (+vi).
- [x] Live F01–F05 claude-haiku-5-5: 5/5, $0.0106, p50 11.9 s; trace totals = per-request sums.
- Follow-ups: false-success detection is phrase-based; no compaction spans.

### G2 — merged (c19d608..e7f6f57)
- [x] Live pilot tasks via `hame eval`: fresh fixture app per run, checker access hidden from the agent, kill-and-resume, teardown. Tasks B01–B05, D01, D03, D04, F01–F05, R01–R05, X01; injection suite I01–I04 (`evals/pilot/injection.toml`).
- [x] Haiku × 3: 56/57, 0 false successes/timeouts/errors, p50 15.1 s, p95 83.6 s, $0.2474. B04 failure = commit refused for lack of grant (authorization as designed). Sonnet 1/1 on B04.
- [x] Faults fired every run: R03 kill after payment + resume (exactly one payment), R04 helper killed during slow load, R05 window raised over agent's, X01 save dialog cancelled.
- [x] Ablations (B01–B05 × 3, interleaved, bootstrap 95% CI): batch off +0.73 requests/attempt [0.13, 1.33], +1.57 s [0.88, 2.31]; screenshot-first +308 KB request bytes, +$0.0007 [0.0006, 0.0009]. `[computer] batch` (default on) and `observation` (default semantic) flags.
- [x] Injection 12/12 held (model ignored planted text; harness block not exercised).
- [x] Bugs fixed with tests: `hame run` killed browser at exit; Save As field accepting a full path (saved to iCloud); keys not reaching TextEdit's Open window; line breaks lost in observed values; typing into a window that took focus; success-claim check reading whole answer.
- Evidence: [EVIDENCE.md](EVIDENCE.md). Total API spend $0.61 (priced from reported usage).
- Limits: n = 3 per task, 5 tasks in ablation; screenshot arm keeps refs (not a pixels-only agent); Haiku over-declares commits.

### F — merged
- [x] `hame workflow list|show|trust|untrust`, doctor checks; README/README.vi, CHANGELOG, SECURITY, CONTRIBUTING; INSTALL, SUPPORT-MATRIX, RELEASE-NOTES, RELEASE-INSTRUCTIONS, THIRD-PARTY-LICENSES, `sbom.cdx.json`, `scripts/license-report.py`, `scripts/release-check.sh`, `scripts/docker-clean-install.sh`, manual `release.yml` (never run). Version 0.1.0-rc.1.
- [x] Clean install: macOS (340 pass), Linux arm64 Docker (339 pass). 432 crates; MPL-2.0 im-rc/bitmaps/sized-chunks (unmodified), WTFPL terminfo.
- Unverified: Linux x86_64, macOS x86_64, Windows, MSRV 1.89, ACP image display in editors, OpenAI/PDF live paths. `command` predicate runs `sh -c` outside the sandbox (documented).

## Test log

| Date | Command | Result |
|---|---|---|
| 2026-11-15 | `cargo test --workspace` at `cc2ac82` | 233 passed, 0 failed, 1 ignored |
| 2026-11-15 | `cargo test --workspace` after shared contracts | 237 passed, 0 failed, 1 ignored; clippy clean |
| 2026-11-15 | `cargo test --workspace` after verification/workflows/grants (1ac63c4) | 243 passed, 0 failed, 1 ignored |
| 2026-11-15 | `cargo test -p hame-verify` after budget + task files (a1d9753) | 20 passed |
| 2026-11-15 | `cargo test --workspace` after merging B | 259 passed, 0 failed, 5 ignored (4 live) |
| 2026-11-15 | `HAME_LIVE_ANTHROPIC=1 cargo test -p hame-llm --test live_anthropic -- --ignored` (B) | 4/4 pass, claude-haiku-5-5 |
| 2026-11-15 | `cargo test --workspace` after merging A | 274 passed, 0 failed, 5 ignored |
| 2026-11-15 | `cargo test --workspace` after merging C and E | 315 passed, 0 failed, 5 ignored |
| 2026-11-15 | `cargo test -p hame-desktop --test gui -- --ignored --test-threads=1` (D) | 11/11 pass, 58 s |
| 2026-11-15 | `cargo test --workspace` after merging D + integration (74efb91) | 336 passed, 0 failed, 16 ignored (live + GUI) |
| 2026-11-15 | `hame task t1.toml --provider anthropic --model claude-haiku-5-5` | verified; 3 requests, 10949 in / 7065 cached / 422 out |
| 2026-11-15 | `cargo test --workspace` after merging F + cost budget (7bafd62) | 340 passed, 0 failed, 16 ignored |
| 2026-11-15 | `hame eval evals/pilot --runs 3 --provider anthropic --model claude-haiku-5-5` (G2, at 74ba6e4) | 56/57 pass; details in EVIDENCE.md |
| 2026-11-15 | `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` after merging G2 | clean; 349 passed, 0 failed, 17 ignored |

### A — merged (c844fef, e3024b3, 2932957)
- [x] Per-session ToolContext (artifacts, journal, session id) derived from `SessionWriter` each turn; `request.artifacts` set.
- [x] Settled receipts copied into the log once per step; on open, unsettled → `outcome_unknown` + interrupted result lists actions to reconcile; cancel treats dispatched actions the same; nothing replayed.
- [x] `check_artifacts`, `gc_artifacts(min_age)` (+dry run), `hame sessions gc`; `put` refreshes mtime.
- [x] Image retention (KEEP_IMAGES=3), masking clears images; compaction appends deterministic "Durable task facts".
- [x] Fork carries settled receipts; import stores base64 images (DeepSeek Harness importer only — no Claude Code/Codex importer exists).
- [x] TUI/web (`/api/artifacts/<sha>?session=`)/ACP image rendering. README retention paragraph.
- Follow-ups: image retention changes older history → breaks prompt cache at each new image (P4 item); KEEP_IMAGES not configurable.

### B — merged (aa23be8, d8e7c19)
- [x] Native images: Anthropic image/document blocks (incl. in tool_result), Responses input_image/input_file (incl. function_call_output arrays), Chat Completions follow-up user message for vision models.
- [x] `ModelProfile` (vision, pdf, limits) resolved per provider/model; `.with_profile()` override; explicit notes for every unsendable image.
- [x] `Completion.meta` filled (request/response bytes, images, image bytes, ttft, model, raw usage); usage no-double-count fixture tests.
- [x] MCP mixed content preserved (text, image→artifact, resources, links, structuredContent→Data, audio→Unsupported).
- [x] Live (claude-haiku-5-5): image in tool result, image in user message, missing image note, prompt cache — 5/5 pass, ≈$0.0013.
- Follow-ups: unknown models default text-only (config override needed — wire `[model] vision`); PDF and OpenAI paths fixture-only.

## Dependency licenses (pre-merge snapshot, `cargo metadata`, all targets)

514 third-party crates. Permissive (MIT/Apache/BSD/ISC/Zlib/Unicode/CC0/Unlicense) except: `im-rc`, `bitmaps`, `sized-chunks` (MPL-2.0, file-level copyleft; unmodified), `terminfo` (WTFPL), `webpki-root-certs` (CDLA-Permissive-2.0). Dual `GPL-2.0-only OR BSD-3-Clause` is used under BSD-3-Clause. Final report after wave 1 merges.

## Release blockers

None recorded yet.
