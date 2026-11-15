# Architecture decisions

Short records for the boundaries the [roadmap](../research/UNIFIED-HARNESS-ROADMAP-2026-11-15.md) asks to fix first. Each says what was decided, why, and what would reopen it.

## ADR-1 Source of truth: the session log plus content-addressed artifacts

The append-only session log remains the source of truth. Binary evidence (screenshots, downloads) goes into a content-addressed artifact store (`<home>/artifacts/<aa>/<sha256>`), written and fsynced before the event that references it. Logs hold only `ArtifactRef`s. The format version became 2; version 1 logs load unchanged, and a version 1 build refuses a version 2 log with an explicit "newer format" error.

Action receipts live in a per-session `actions.v1.jsonl` journal beside the log, because adapters write them from inside a tool call while the log belongs to the agent loop. Settled receipts are copied into the log as `action/receipt` events. Reopen if indexing needs justify a derived store.

## ADR-2 hame owns the task

Task contract, authorization, budgets, target identity, cancellation and completion are hame's. Adapters are bounded executors behind the same tool pipeline as every other tool. No nested autonomous agent loop runs inside an adapter; adapters declare `NestedUsage::None`.

## ADR-3 Adapters are versioned subprocess helpers

Browser and desktop backends run as long-lived child processes speaking line-delimited JSON with a version handshake (`AdapterManifest`, `CONTRACT_VERSION`). This isolates crashes and native code from the hame process. Reopen if measured IPC overhead is material.

## ADR-4 Browser backend: direct Playwright helper (decided after comparison in workstream C)

Candidates: vercel-labs agent-browser and a direct Playwright adapter. Decided: the direct Playwright helper (`crates/hame-browser`). On identical fixture steps agent-browser 0.41.0 was faster per click and fill (fewer checks) but reported a date fill that did not take, and re-targeted old refs to re-rendered or relabelled elements without saying so. Evidence, method and raw numbers: [EVIDENCE-browser.md](EVIDENCE-browser.md).

## ADR-5 Desktop backend: native macOS accessibility helper (decided after comparison in workstream D)

Candidates: vercel-labs agent-desktop, Microsoft HCU, and a small Swift helper over AX/CoreGraphics. Decided: the Swift helper (`crates/hame-desktop`) is the only default. On the same TextEdit window it observed in 8.5 ms median vs 137 ms for agent-desktop (which pays a process start per command) and reported `stale_window` for a closed window where agent-desktop reported `APP_UNRESPONSIVE` with a retry hint. HCU's provider needs its own Accessibility grant, which would have changed the user's privacy settings, so it was not run. Evidence: [EVIDENCE-desktop.md](EVIDENCE-desktop.md). Requires macOS 14+ (screenshot API).

## ADR-6 Vision is selective and typed; no default local model

Images travel as typed blocks end to end. Screenshots are taken per window or region on request, with the capture transform recorded. No local vision model is on the default path; none was shown to improve the quality/latency/cost frontier on this hardware.

## ADR-7 Desktop authority is separate from the shell sandbox

Browser and desktop tools are host-side and act outside the shell sandbox. They are governed by explicit grants (`Authorization`: channel × app/origin/profile × capability) from configuration or the user, enforced at the adapter and in the policy pipeline. Observed content (pages, accessibility labels, screenshots) is marked untrusted and can never grant scope.

## ADR-8 Effects are journalled before dispatch

Every action that can change the world is journalled `not_started` → `dispatched` → settled. After a crash, dispatched actions become `outcome_unknown` and an action with the same idempotency key is refused until the state is reconciled by a fresh observation. Conversation replay never replays effects.
