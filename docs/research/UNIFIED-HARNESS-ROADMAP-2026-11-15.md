# Unified harness: implementation and evaluation roadmap

Date: **15 November 2026**. Based on [the research report](UNIFIED-HARNESS-RESEARCH-2026-11-15.md) and local hame revision `cc2ac8278ff85e7938ae169e3825a616742f7426`.

This is a proposed engineering plan. It does not describe changes already implemented or authorize spending on model APIs, cloud machines or external services.

## 1. Pilot outcome and assumptions

Deliver a hame pilot that completes an explicitly supported set of browser, file, document and native-desktop workflows from one user instruction. The pilot must select appropriate execution channels, verify the final result, expose failures accurately, and recover without duplicating external effects.

Use **two experienced engineers over approximately 16 weeks** as a planning assumption: one focused on runtime/persistence and one on adapters/evaluation, with shared review. This is an estimate, not a commitment. Existing adapter quality, application accessibility and operating-system packaging can change it substantially. For a single developer, follow the same dependency order and reduce application/platform coverage before weakening verification.

Use macOS as the first host-desktop pilot because the inspected development environment is macOS. Use a resettable Linux browser/desktop environment for repeatable experiments where practical. Validate Windows and Linux adapter feasibility early; expanding production support is gated by real tests, not an upstream cross-platform label.

The first end-to-end demonstration is:

> Read a report from a test web application, export its data, calculate a summary locally, create a formatted report, open it in a desktop application, and save the verified result to the requested folder.

The fixture includes a deliberately changed page, an unexpected modal and an interrupted save. Success requires correct values and formatting, the correct output location, and no duplicate operation.

## 2. Architectural decisions to record first

Create short architecture decision records for these boundaries before implementation spreads across crates:

| Decision | Initial recommendation | Reconsider when |
|---|---|---|
| Source of truth | Existing append-only session events plus content-addressed artifacts | Measured indexing needs justify an additional derived store |
| Execution ownership | hame owns task, budget, permissions and completion | A delegated external service has a distinct bounded job and observable lifecycle |
| Adapter deployment | Versioned subprocess/IPC helpers | Profiling shows IPC or startup is material and an FFI integration is worth its risk |
| Browser backend | Compare agent-browser and a direct Playwright adapter; select one | Coverage or maintenance evidence favors switching |
| Desktop backend | Compare agent-desktop and HCU; select one primary backend | Required apps fail the chosen backend's coverage tests |
| Vision | Typed image transport and selective screenshots | Semantic coverage is insufficient or the outcome requires visual evidence |
| Local models | Optional, off the default path until measured | Hardware-specific evaluation demonstrates a better quality/latency frontier |
| Workflow reuse | Versioned scripts with predicates and explicit scope | A task changes enough that replanning is cheaper than maintenance |

The technical evidence for these choices and their source links are in the [research report](UNIFIED-HARNESS-RESEARCH-2026-11-15.md). The [source register](UNIFIED-HARNESS-SOURCES-2026-11-15.md) supplies revision pins.

## 3. Milestones

| Phase | Indicative time | Deliverable | Exit condition |
|---|---|---|---|
| P0 | Week 1 | Trace schema, pilot fixtures, task/target/action contracts | Every controlled run has outcome and resource accounting; unsupported observations are explicit |
| P1 | Weeks 2–3 | Typed artifacts and multimodal transport | Image survives tool → log → model → resume; text-only behavior remains compatible |
| P2 | Weeks 4–5 | Persistent browser execution | Five browser workflows pass hidden outcome checks and stale-state/interruption cases |
| P3 | Weeks 6–8 | Native desktop execution plus selective vision | Supported macOS app matrix, correct coordinates and scoped broker behavior demonstrated |
| P4 | Weeks 9–11 | Batching, events, compact observations and routing experiments | Controlled ablations identify gains without observed quality loss; costs include nested inference |
| P5 | Weeks 12–13 | Reusable workflows and cross-app verification | Repeated workflows generalize to held-out inputs and stop correctly on drift |
| P6 | Weeks 14–16 | Hardening, larger held-out evaluation and pilot packaging | Quality/resource gates met on the stated support matrix; limitations and rollback documented |

The semantic-browser prototype can be explored while P1 is in progress. Production integration must wait for shared target, permission and receipt contracts. Full three-OS parity is a separate deliverable if it cannot fit the pilot without reducing quality.

### P0 — Measurement and contracts

Extend the existing session and evaluation infrastructure instead of building a separate agent harness.

- Define task, target, observation, action-plan and receipt schemas, with explicit schema versions.
- Add correlated monotonic spans for model requests, observation capture, preprocessing, adapter calls, waits, verification and recovery.
- Preserve raw provider usage and normalize uncached input, cached reads, cache writes and output. Keep unavailable reasoning/vision breakdowns as unknown.
- Add actual wire-byte counts and image dimensions. Do not infer upload size from token usage.
- Track model calls inside delegated adapters. If unavailable, mark totals incomplete and exclude the run from claims of total token reduction.
- Measure agent execution, final checker time and complete wall time separately. The existing evaluator currently measures the agent before invoking the hidden checker.
- Freeze an initial task manifest, reset procedure, acceptance predicates and test accounts.

**Exit evidence:** a machine-readable trace for a successful task, a failure, a timeout and an interrupted action; a report that includes every attempt; no secret content in ordinary telemetry. Pure telemetry can use a separate trace stream referencing durable event IDs; it does not need to inflate the prompt.

### P1 — Images, structured results and artifacts

The immediate code changes span [hame-tools](../../crates/hame-tools/src/lib.rs), [hame-session](../../crates/hame-session/src/model.rs), [hame-ext MCP](../../crates/hame-ext/src/mcp.rs), [hame-llm](../../crates/hame-llm/src/lib.rs), context projection and the frontends.

- Introduce typed text, image, artifact-reference and structured-data results. Keep a simple text helper for existing tools.
- Add atomic artifact writes, content hashes, MIME/size validation and explicit permission scopes for artifact access.
- Record capture target, timestamp, coordinate space and transforms. Separate original bytes from any redacted or resized derivative.
- Preserve MCP text, images and structured data together. Reject or explicitly represent unsupported blocks rather than silently losing them.
- Serialize images through supported provider formats; enforce model capability checks.
- Define a session format migration and old-reader behavior. Update replay, forks, imports, exports, compaction and UI rendering.
- Make missing or deleted artifacts detectable during resume. Document retention rather than accumulating full-resolution screenshots forever.

**Exit evidence:** fixture-based provider serialization; image-plus-text MCP round trip; old text logs still load; a crash between artifact write and event append is recoverable; compaction preserves task facts and artifact provenance. Use small deterministic fixtures before any paid live model smoke test.

### P2 — Browser adapter and first complete task

Compare pinned agent-browser and direct Playwright implementations on the same fixture app. Both must expose the same hame-facing contract so the comparison measures backend behavior rather than different planner prompts.

- Keep a browser session alive across calls, with explicit profile and ownership.
- Return task-focused semantic observations with bounded expansion.
- Support navigation, forms, extraction, file upload/download and outcome checks.
- Scope refs to tab/document generation and observation; invalidate on navigation and material changes.
- Add actionability checks, event-based waits, timeouts and typed errors.
- Bind origin/account/profile and allowed destinations to authorization.
- Make cancellation and reconnection observable; an old tab ref cannot silently attach to a new page.

**Exit evidence:** the five browser tasks below; duplicated labels; hidden controls; a DOM mutation between observation and action; a delayed download; wrong-profile rejection; a crash after fixture submission with reconciliation and no duplicate submission.

Choose the backend on correctness and coverage first, then latency, token footprint, operating cost and maintenance. Avoid maintaining two default browser state machines after the experiment.

### P3 — Native desktop and visual fallback

Compare agent-desktop and HCU for native control. Start with a small set of applications such as file manager, text editor, office application and system preferences. Publish exactly which operations and versions were tested.

- Add a desktop broker distinct from arbitrary shell execution.
- Expose discovery, target-window selection, semantic observation, scoped actions and optional screenshot capture.
- Implement window/process identity, observation generations and live preconditions.
- Track action capability requirements such as focus, global input, clipboard, file writes and network-bearing app actions.
- Bind physical input to a desktop lease. Keep semantic operations that do not require focus separate.
- Detect human interference and unexpected modal/window transitions.
- Validate capture transforms under multiple monitor origins, display scaling, resized windows and crops.
- Return “unsupported” for inaccessible surfaces; route to visual control explicitly.
- Keep emergency stop and held-key/button cleanup close to the native execution process.

**Exit evidence:** stale window and ambiguous target rejection; Unicode/Vietnamese input; clipboard state handling; denied screen/accessibility permissions; blank capture detection; cancellation during a batch; declared behavior on each tested platform.

The existing Windows sandbox confines writes only under its documented partial mode. Desktop integration must not be described as fully confined merely because the driver runs in a worker. Treat an attached application's authority separately.

### P4 — Optimize using ablations

Implement improvements behind independent feature flags so each mechanism can be measured and rolled back.

1. Bounded action sequences with a completed-prefix receipt, local assertions and immediate stop conditions.
2. Event-driven waits with timeout fallback; avoid unconditional sleeps as the primary synchronization method.
3. Semantic subtree selection and diff tracking with base-version validation.
4. Target-window capture, regions of interest and adaptive image dimensions/encoding.
5. Observation retention that preserves current state, evidence and task constraints.
6. Task-aware channel selection, followed by model/effort routing as a separate experiment.
7. Optional local OCR or grounding on explicitly identified hardware.

**Exit evidence:** arms A–E from the research report, matched by task/model/settings; per-stage timings; p50/p95; strict completion and false-success counts; recovery overhead; complete inference and compute cost. Evaluate bandwidth and RTT separately. Keep a local model disabled by default if it only shifts cost to the user's machine or creates more retries.

Batching is a local execution optimization, not permission to ignore state changes. Every inner action still goes through policy and emits its own receipt. Local checks should run after each meaningful change; the cloud planner only needs to be invoked at a decision boundary or exception.

### P5 — Reusable workflows and domain verification

- Store a parameter schema, required capabilities, authorized effects, preconditions, postconditions, tested application versions and provenance for each workflow.
- Begin with five repeated workflows from real pilot demand.
- Use selectors and semantic state where possible; avoid recording absolute click coordinates as a reusable recipe.
- Revalidate authorization and relevant live state on every invocation.
- Maintain a small test corpus of input variations and UI changes for each workflow.
- Invalidate or repair a workflow when its preconditions fail. Report repair inference and failed-run cost.
- Generalize the verification interface beyond shell checks: artifact, browser state, application state and remote operation receipts.

**Exit evidence:** correct cold and warm execution on held-out input variations; intentional failures after UI drift; no silent change of account/destination; no automatic promotion of a single successful trace into trusted procedural memory.

Offline interface generation inspired by Agent S4 can follow this phase. Its output must pass held-out regression tests before entering the trusted workflow catalog. Training and selection must not use the final evaluation answers.

### P6 — Harden and package the supported product

- Expand from diagnostic tasks to a larger held-out release suite.
- Determine sample size for the selected quality margin; publish paired results and confidence intervals.
- Test provider errors, connectivity loss, malformed adapter output, mid-write crashes, stale state, prompt injection in observations and human interference.
- Package pinned helpers with explicit capability/version negotiation and upgrade tests.
- Maintain an SBOM and inspect component/model terms for the exact distributed artifacts. GitHub's detected root license is insufficient for mixed repositories.
- Document supported apps/platforms, known unsupported operations, local resource requirements and cost-accounting gaps.
- Run an opt-in pilot, collect consented/redacted traces, and use failure clusters to prioritize the next increment.

**Release gate:** no known correctness blocker in supported workflows; scoped authorization and recovery cases pass; no observed quality regression against the matched baseline; speed/token claims include their confidence and workload boundaries. If evidence is insufficient, retain the feature flag and expand evaluation.

## 4. First ten working days

| Day | Concrete output |
|---|---|
| 1 | Baseline revision, dependency pins, task contract and explicit success definitions |
| 2 | Model/tool/verification trace events and normalized usage records |
| 3 | Resettable browser fixture and artifact validator; failure/timeout reporting |
| 4 | Typed content and artifact-store design, including migration and retention |
| 5 | First end-to-end image transport fixture and missing-artifact handling |
| 6 | MCP mixed-content preservation and provider request fixtures |
| 7 | Prototype semantic browser adapter with a persistent session |
| 8 | Native driver feasibility matrix, permission setup and ref-invalidity probes |
| 9 | Initial failure injection: stale target, modal, interrupted dispatch and resume |
| 10 | Reviewed evidence pack and revised estimates based on measured adapter coverage |

These days are a sequencing guide. Move unresolved correctness work forward before committing to an optimization baseline. Obtain a defined spending cap and test-account scope before running paid or externally stateful evaluations.

## 5. Initial 30-task diagnostic manifest

All tasks use controlled fixtures or dedicated accounts. Each fixture should include initial state, task prompt, allowed channels, expected result, forbidden side effects, reset procedure and a checker hidden from the agent.

| IDs | Task | Checker |
|---|---|---|
| F01 | Rename a function across a small repository | Hidden caller/tests; unrelated files unchanged |
| F02 | Repair a broken CSV transformation | Output equals hidden expected data |
| F03 | Organize files by supplied rules | Exact paths, hashes and duplicate handling |
| F04 | Generate a report from JSON inputs | Required fields, totals and source references |
| F05 | Update configuration without overwriting local changes | Parsed values and preserved user content |
| B01 | Complete a form with duplicate field labels | Correct fixture record and no extra submission |
| B02 | Extract a filtered, paginated table | All and only matching records |
| B03 | Upload an artifact to a specified test record | Attachment hash and target ID |
| B04 | Download the correct date-range export | CSV contents and download completion |
| B05 | Change a preference in the requested browser profile | Correct profile/account and persisted state |
| D01 | Save a text document to a specified folder | Reopened bytes and location |
| D02 | Change a desktop application preference | Reopened setting |
| D03 | Navigate a native file dialog with similar filenames | Correct chosen file hash |
| D04 | Enter Vietnamese text into a native app | Unicode-preserving persisted text |
| D05 | Adjust a document's appearance | Structural fields plus selected visual checks |
| O01 | Populate a spreadsheet from supplied data | Cell values and row completeness |
| O02 | Add formulas and recalculate totals | Formula structure and calculated values |
| O03 | Update a document while preserving existing formatting | Text, styles and rendered comparison |
| O04 | Create a chart with correct labels and units | Data mapping plus visual verification |
| O05 | Export a document and reopen the exported file | Format validity, content and required appearance |
| X01 | Browser export → local transform → desktop report | End-to-end data lineage and saved artifact |
| X02 | Gather fixture records → spreadsheet → upload | Record coverage and destination hash |
| X03 | Edit a file, preview in an app, export | Content, appearance and output location |
| X04 | Compare two data sources and write discrepancies | Correct joins and discrepancy set |
| X05 | Update a report in response to a mid-task user correction | Final output reflects the latest instruction |
| R01 | Unexpected modal during a planned sequence | Stops/re-observes and completes correctly |
| R02 | Target changes after observation | No input to stale target |
| R03 | Crash after a fixture submit | Reconciles without duplicate write |
| R04 | Slow page or disconnected adapter | Bounded wait, accurate status and recovery |
| R05 | Human changes focus during physical input | Stops or revalidates; no typing into wrong window |

Prompt-injection and authorization tests should be an additional cross-cutting suite, not counted as ordinary task success. Place malicious instructions in fixture page text, document content and accessibility labels; verify that they cannot expand permissions or change the requested destination.

## 6. Proposed code ownership and boundaries

| Existing area | Change |
|---|---|
| `hame-session` | Typed content, observation/receipt events, task facts, format migration |
| `hame-tools` | Rich result type, explicit target/effect metadata, adapter-facing operations |
| `hame-llm` | Multimodal serialization, capability profiles, raw/normalized usage and request spans |
| `hame-ext` | MCP rich results, lifecycle wrappers, script/workflow integration |
| `hame-context` | Observation-aware projection, artifact references, durable task facts |
| `hame-policy` / `hame-sandbox` | Capability grants, target scopes, desktop broker boundary, secret handles |
| `hame-verify` | Domain-neutral postconditions and evidence-bearing completion |
| `hame-agent` / `hame-app` | Scheduler, target leases, routing, cancellation and runtime assembly |
| `hame-cli` / `hame-eval` | Browser/desktop fixtures, fault injection, complete task timing and comparison output |
| Frontends | Artifact display, task status, concrete permission boundaries and immediate stop |

Create a separate `hame-computer` or `hame-observe` crate only when implementation size and dependencies justify it. Start with clear module contracts rather than speculative crate proliferation.

## 7. Operational evaluation record

Every reported experiment should contain:

```text
experiment_id, task_set_version, task_id, run_index
hame_commit, adapter_commits, model_version, prompt_version
os, hardware, app_versions, viewport, display_scale
network_profile, initial_state_id, cold_or_warm, enabled_features
outcome, failure_category, forbidden_effects, human_interventions
agent_ms, verification_ms, total_ms, stage_spans
uncached_input, cached_input, cache_write, output, reasoning_if_exposed
model_calls, nested_model_calls, usage_complete
request_bytes, image_bytes, screenshots, semantic_observations
local_compute_time, peak_memory, billed_cost, tariff_date
artifact_hashes, receipt_ids, trace_reference
```

Reasoning tokens can be a subset of output tokens; vision tokens can be included in input totals. Preserve provider definitions and do not sum overlapping fields. Record retry and failed-attempt costs. Report total cost across all attempts divided by verified successes as well as per-attempt cost. If there are no successes, the first metric is undefined, not zero.

Pin every experiment. Repeat broad performance measurements only after a meaningful change or unresolved finding; use targeted conformance tests for routine adapter upgrades. Weekly review should focus on the largest failure and latency categories, with explicit decisions to keep, change or remove each optimization.

## 8. Scope after the pilot

Expand applications and operating systems through the same evidence gates. Add service connectors when they remove expensive UI work and preserve the user's requested semantics. Introduce offline interface evolution, sophisticated model routing or remote delegated agents only after traces show that they address a material bottleneck.

The valuable long-term asset is a tested collection of execution capabilities and reusable procedures operating under one reliable runtime, backed by outcome evidence and complete resource accounting.
