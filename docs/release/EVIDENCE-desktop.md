# Desktop backend comparison (ADR-5)

Workstream D, 2026-11-15. Time-boxed to 30 minutes of comparison work after the hame helper was implemented.

## Question

Which backend should drive native macOS applications for hame: vercel-labs agent-desktop, Microsoft Hybrid-Computer-Use (HCU), or the small Swift accessibility helper in `crates/hame-desktop`? One primary backend, one default implementation.

## Environment

| Item | Version |
|---|---|
| Machine | Apple M5, built-in Liquid Retina XDR display (3024 x 1964, scale 2.0), single display |
| OS | macOS 26.6.2 (25G83) |
| Swift | Apple Swift 6.4.2 (swiftlang-6.4.2.1.9) |
| Rust | rustc 1.99.1 (agent-desktop built with its pinned 1.89.0) |
| Go | go1.26.4 darwin/arm64 (for HCU) |
| Test app | TextEdit 1.20 (build 415), a new untitled rich-text document, in the background |
| Permissions | Accessibility and Screen Recording granted to the terminal process tree |
| hame helper | `crates/hame-desktop/helper/DesktopHelper.swift` at this commit, `swiftc -O -swift-version 5` |
| agent-desktop | vercel-labs/agent-desktop rev `46d2adf885002633b77d5a9956c9464b24f61431` (v1.3.1, envelope 2.6), `cargo build --release --locked -p agent-desktop` |
| HCU | microsoft/Hybrid-Computer-Use rev `697d3fbda04cb322915866bab1aad79c726a61e3` (v1.0.0), `make build-macos` |

## Method

`crates/hame-desktop/bench/compare_backends.py` creates one TextEdit document and, on that same window, runs each step 12 times per backend, measuring wall-clock time per operation from the calling process (median and p95, nearest-rank):

1. observe the document window (hame `observe`, max 150 nodes; agent-desktop `snapshot --app TextEdit --window-id w-<id> -i`);
2. set the text area's value to alternating Vietnamese strings (`Tiếng Việt có dấu: ắ ằ ẳ ẵ ặ đ Đ ơ ư`, `Đ ơ ư ắ ằ ẳ ẵ ặ — lần hai`) and read it back through accessibility to check it is exact;
3. type the Vietnamese string once into the emptied text area;
4. take a ref, close its window, and act on the stale ref.

hame is measured at the helper (one long-lived process, JSON lines over a pipe). agent-desktop is a CLI that runs one process per command, so its numbers include process start, measured separately with `agent-desktop version`. The hame tool as a whole (authorization, rendering, fsynced receipts) is measured by the `tool_level_latency` GUI test.

The script closes its documents, quits TextEdit if it was not running, and agent-desktop's state directory (`~/.agent-desktop`, created by the runs) was deleted afterwards.

## Results

### Correctness on the same steps

| Check | hame helper | agent-desktop |
|---|---|---|
| set value, Vietnamese, exact read-back | 12 / 12 | 12 / 12 |
| type Vietnamese | exact (physical key events, chunks of 8, each chunk confirmed) | exact (semantic insert in its default headless mode) |
| act on a ref after its window closed | `stale_window`: "Window 5470 of TextEdit no longer exists; it was closed or replaced. Observe again." | `APP_UNRESPONSIVE`: "Accessibility surface read failed for AXRole", suggestion "Retry after the application finishes updating its accessibility surfaces" |

agent-desktop's stale-window answer is misleading: it classifies a closed window as an unresponsive app and suggests a retry, which invites a loop instead of a fresh observation. (Its documentation does promise `STALE_REF` for changed elements; that path was not exercised in the time box.)

### Latency, final run (ms, n = 12)

| Operation | hame helper median | p95 | agent-desktop median | p95 |
|---|---:|---:|---:|---:|
| observe / snapshot | 8.5 | 12.4 | 137.3 | 159.2 |
| set value | 1.6 | 5.6 | 29.5 | 33.8 |
| process start (`version`) | n/a (long-lived) | | 6.0 | 8.1 |
| type 39 Vietnamese characters (single run) | 155 | | 33 | |

Earlier runs on the same day, before the helper batched its attribute reads: hame observe 9.7 / 21.8 and 16.7 / 21.6, set value 1.2 / 4.6 and 2.1 / 3.6; agent-desktop snapshot 110.8 / 155.4 and 118.3 / 131.0, set-value 25.7 / 31.3 and 25.6 / 32.0.

hame's `type` is slower by design: it posts real key events in chunks and, before each chunk, re-checks the frontmost app, focused window, focused element and sheets, then waits until the app has taken the chunk. That is what lets it stop on interference without typing into the wrong window. agent-desktop's default `type` inserts text through accessibility, which is fast and needs no focus but is not keyboard input (apps that react to key events do not see it).

### hame tool level (debug build, ms, n = 12; two runs: final, then the one before)

| Operation | median | p95 |
|---|---:|---:|
| `windows` | 1.9 (1.9) | 2.3 (3.2) |
| `observe` | 9.3 (10.3) | 14.1 (11.9) |
| `set_value`, journalled | 23.1 (29.6) | 31.0 (35.0) |
| `set_value`, no journal | 7.8 (8.5) | 8.6 (9.8) |

About 15 to 21 ms of every acting operation is the action journal: three fsynced writes (`not_started`, `dispatched`, settled), and Rust's `sync_data` on macOS is `F_FULLFSYNC`. The remaining 8 ms is the live resolve plus the action. This is a property of the shared `ActionJournal` contract, not of the backend; see "Open issues".

### HCU

HCU builds on this machine (Go core plus a Swift provider), but it cannot run under the permissions available here. Its macOS provider is a separate app bundle, `ComputerUseProvider.app` (bundle id `org.computer-use.provider.v2`, ad-hoc signed), which the CLI starts through LaunchServices (`/usr/bin/open -n ... --args --socket ...`). It is therefore its own privacy (TCC) identity and does not inherit the terminal's grants. The first live call:

```text
$ computer-use call AX --args '{"mode":"apps"}'
{"ok":false,"code":"runtime_error","message":"Accessibility permission is unavailable. Enable Accessibility for Computer Use Provider.","input_emitted":false,"preflight_only":true}
```

Granting Accessibility and Screen Recording to that bundle means changing Privacy & Security settings, which this run was not permitted to do, and every rebuild of the ad-hoc signed bundle may need the grant renewed (its own documentation says so). No HCU numbers were taken. Its design (live preflight of every AX target and GUI coordinate, `effect_unknown` on unverifiable results, batches of up to 64 actions) is close to hame's and remains a useful reference.

## Decision

**The hame Swift helper (`crates/hame-desktop`) is the primary and only default desktop backend.** agent-desktop and HCU are not wired in.

Reasons, in order:

1. It speaks hame's contracts directly: `Target` with CGWindowID, pid and launch generation; refs scoped to an observation; `Authorization` checked per capability before dispatch; receipts journalled before dispatch; the desktop lease for physical input; `Capture` transforms for screenshots. With either alternative these would be a translation layer over a different model of identity and refs.
2. It reports the failure modes hame needs to tell apart: `stale_window`, `stale_ref` (including elements of a cancelled sheet that the app keeps alive), `ambiguous`, `interference`, `unexpected_dialog`, `cancelled` with the emitted prefix. agent-desktop reported a closed window as `APP_UNRESPONSIVE` with a retry hint.
3. It runs under the grants of the process that runs hame. HCU needs its own privacy grant per provider bundle, and agent-desktop keeps state in `~/.agent-desktop`.
4. It is faster per operation (observe 8.5 ms vs 137 ms; set value 1.6 ms vs 29.5 ms), because it is one long-lived process.
5. It is one Swift file (about 1,600 lines) compiled on first use; no Go, Node or extra toolchain.

What agent-desktop does better and could be borrowed later: broader operations (scroll, select, expand, drag, notifications, menu-bar extras, Windows and Linux backends) and a focus-free semantic text insertion mode.

What would reopen the decision: hame needing Windows or Linux desktop control soon (agent-desktop and HCU both have backends), or the required app matrix (see `crates/hame-desktop/README.md`) failing on the helper in ways those backends handle.

## Open issues

- The action journal's three `F_FULLFSYNC` writes cost about 15 to 21 ms per acting operation on this machine, more than the operation itself. Writing `not_started` and `dispatched` as one record, or a cheaper barrier for the first, would cut that without weakening "journalled before dispatch". This belongs to the shared contract (`hame_session::contract::ActionJournal`).
- agent-desktop's `STALE_REF` path and its long-lived MCP mode were not measured within the time box.
- HCU was built but not run (permission identity, above).

## GUI test run (hame helper)

`cargo test -p hame-desktop --test gui -- --ignored --test-threads=1` on this machine, 2026-11-15, final helper source: **11 passed, 0 failed** in 58 s. TextEdit was not running before and was quit afterwards; every document was created and closed by the tests, files were saved only into temporary directories.

| Test | What it shows |
|---|---|
| `d01_textedit_new_document_saved_through_the_save_panel_beside_a_decoy` | New document, `set_value`, File > Save…, Go To Folder sheet, save next to `hame-d01 copy.rtf`; RTF file text verified with `textutil`, decoy bytes unchanged; the confirming Save is refused without FileWrite; receipts `not_started`/`dispatched`/`completed` with an idempotency key; a repeated File > Save is refused (`prior_effect`) until reconciled after a newer observation |
| `d04_vietnamese_text_persists_exactly_through_set_value_and_type` | `Tiếng Việt có dấu: ắ ằ ẳ ẵ ặ đ Đ ơ ư` saved exactly via `set_value` and via `type`; the type receipt records exactly the typed text |
| `stale_refs_and_closed_windows_are_rejected` | The Save button of a cancelled sheet is `stale_ref` (the app keeps the element alive; the attachment check catches it); a ref into a closed window is `stale_window`; nothing refused is journalled |
| `two_windows_with_the_same_title_are_ambiguous` | Two `same.txt` windows: `ambiguous` with both candidates; a bare role matching several buttons: `ambiguous`, nothing pressed |
| `typing_stops_when_another_window_takes_focus` | Another process raises window B mid-`type`: `interference`, the emitted prefix is reported and is exactly A's text, B stays empty, the receipt is `outcome_unknown` with that prefix |
| `a_batch_is_cancelled_between_chunks_and_reports_its_prefix` | `CancelHandle::cancel` during the `type` step: 1 of 3 steps completed, prefix reported, the third step never ran; each step went through the pipeline as its own request |
| `a_batch_stops_at_an_unexpected_save_sheet` | File > Close on an unsaved document opens the keep/delete sheet: the batch stops with `unexpected_dialog` after 2 of 3 steps and returns a fresh observation showing the sheet; a failed `expect` stops a batch with `assertion_failed` |
| `screenshots_are_scaled_correctly_and_click_at_maps_into_the_window` | Window screenshot is crop x scale 2.0 pixels, PNG, non-blank; `max_edge` 600 keeps the crop; `click_at` through the downscaled image toggles the Bold button; out-of-image points are `invalid`; after Window > Zoom the old screenshot is `stale_capture` |
| `the_clipboard_is_saved_and_restored` | Vietnamese text set and read back, previous clipboard restored exactly; refused without a Clipboard grant |
| `a_missing_accessibility_permission_is_reported_explicitly` | With the helper told to report Accessibility as untrusted: `permission_denied` naming System Settings > Privacy & Security > Accessibility |
| `tool_level_latency` | The tool-level numbers above |
