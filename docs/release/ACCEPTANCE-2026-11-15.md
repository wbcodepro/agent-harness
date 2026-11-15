# Acceptance review — 2026-11-15

**Decision: HOLD public-release acceptance.** The implementation builds and its regular tests pass, but independent probes reproduce a protected-file disclosure through outcome verification and two ways to verify an incorrect result. These findings need fixes before the release can meet its accuracy and confinement requirements.

Reviewed commit: `497fe24f3d10383466fb8e0f1ae545c4d6d6625a` (`0.1.0-rc.1`). Review host: macOS 26.6.2, arm64, rustc 1.99.1; the declared minimum Rust 1.89.0 was checked separately. This review adds evidence and reproduction material only; it does not change implementation or publish a release.

Evidence: [review files](evidence/acceptance-2026-11-15/README.md), [machine-readable results](evidence/acceptance-2026-11-15/results.json).

**Concurrent-edit boundary:** the workspace tests, live/API and GUI checks, original defect probe and release build finished before 14:00 local time. While the report was being written, additional uncommitted edits appeared in desktop, sandbox and other runtime files. HEAD remained `497fe24`, but those later working-tree changes are not covered by the recorded acceptance checks. They were not reverted or modified by this review. Revalidate a settled revision after integration; this report must not be used to certify the changing working tree.

## Findings requiring changes

### A1 — P1: file acceptance predicates bypass read confinement and disclose protected content

Location: [`outcome.rs:133`](../../crates/hame-verify/src/outcome.rs#L133), with feedback at [`outcome.rs:423`](../../crates/hame-verify/src/outcome.rs#L423).

`FileContains` reads the requested path in the host process with `std::fs::read`, following symlinks. On a mismatch, its evidence includes the first 160 bytes of that file. `OutcomeGate` places this evidence into model-bound feedback; the agent loop appends it as a user message. The `Confinement` callback passed to verification is used for command predicates, not these file reads. `JsonValue`, `FileSha256` and `CsvShape` also read from the host without applying the worker's read policy.

Reproduction uses a trusted contract checking ordinary output `out.txt`, with an expected marker. A sandboxed command cannot read a synthetic `.env` fixture, but can create `out.txt -> .env`. When the outcome check runs, the fake private marker appears in `OutcomeGate.before_stop` feedback. The contract did not authorize reading `.env`; only its output path was changed by the agent. This makes a model mistake or malicious instruction capable of crossing the advertised sandbox boundary.

Observed:

```text
protected_file: sandbox_read_denied=true, sandbox_symlink_created=true, marker_in_model_feedback=true
```

The probe used a fake marker, not a real credential, and did not send that marker to an external model. Real API tests were a separate set of synthetic image/cache fixtures.

Required correction: apply the same effective read policy to every file predicate, including symlink resolution and concurrent path replacement. Legitimate outputs outside the workspace need explicit trusted read authority. Prevent disallowed content from entering evidence or model feedback; redaction alone does not repair the read boundary. Add a regression test that creates the link from a confined worker and verifies that the protected marker never reaches feedback.

### A2 — P2: `FileAbsent` treats an inability to inspect the path as success

Location: [`outcome.rs:125`](../../crates/hame-verify/src/outcome.rs#L125).

`Path::exists()` collapses metadata errors into `false`, so the predicate returns `passed=true` for an uncheckable path. A self-referencing symlink reproduces this without changing permissions or using privileged access: `metadata()` reports `FilesystemLoop`, `symlink_metadata()` confirms an entry exists, but verification says the path is absent. Permission and traversal errors are similarly indistinguishable from absence in this implementation.

```text
file_absent: metadata_error=FilesystemLoop, actual_directory_entry_exists=true, verified=true
```

Required correction: handle metadata results explicitly and fail verification on errors other than the defined absence case. Define whether the condition means removal of the directory entry or absence of a resolved target, then handle dangling links consistently with that definition. Add tests for a missing path, a present file, a dangling link, a cyclic link and an unreadable parent.

### A3 — P2: CSV verification counts physical lines instead of records

Location: [`outcome.rs:183`](../../crates/hame-verify/src/outcome.rs#L183).

The CSV condition splits the entire file with `text.lines()`, parses only the first line as a header, and counts remaining nonempty lines. A valid quoted field can contain a newline. The probe's CSV has one data record, but the verifier rejects a requirement of one record and accepts an incorrect requirement of two. A row missing a declared column also passes the present shape check.

```text
csv_multiline: actual_records=1, requested_records=1, verified=false
csv_multiline: actual_records=1, requested_records=2, verified=true
csv_ragged: data_columns=1, header_columns=2, verified=true
```

Required correction: parse CSV records with a proper parser, validate record structure and report parse failures. Cover quoted newlines, escaped quotes, CRLF, malformed quoting and inconsistent record widths. This needs a correctness fix, not a looser acceptance condition.

## Checks performed on the reviewed source

| Check | Observed result |
|---|---|
| `cargo fmt --all -- --check` | Pass |
| `cargo clippy --workspace --all-targets --locked -- -D warnings` | Pass |
| `cargo test --workspace --locked` | 380 passed, 0 failed, 19 ignored, across 63 test-result blocks |
| Live Anthropic tests, enabled explicitly with the existing `.env` key | 4 passed; 5 real API requests using `claude-haiku-5-5` |
| Ignored native GUI tests, serialized | First run: 13 passed, 1 failed during TextEdit launch, before the test body |
| Targeted rerun of `a_batch_stops_at_an_unexpected_save_sheet` | Passed in 4.66 seconds |
| `cargo +1.89.0 check --workspace --locked --offline` | Pass; minimum compiler check, not an MSRV test-suite run |
| `cargo build --release --locked -p hame-cli` | Pass; executable reports `hame 0.1.0-rc.1` and displays help |
| Independent verifier probe | All three findings reproduced; the probe expects the vulnerable behavior on this revision |
| Existing `dist/SHA256SUMS` | Both listed archive hashes match |

The 19 tests ignored by the regular workspace run were 14 GUI tests, four live Anthropic tests and one live MCP discovery test. The GUI and Anthropic tests were exercised separately; the MCP discovery test remains unrun. The GUI launch failure was `_LSOpenURLsWithCompletionHandler ... error -600` at `gui.rs:177`. Its targeted rerun passed, so this is evidence of a startup flake, not proof that the batch/dialog feature is broken. It would be inaccurate to report the original full GUI run as 14/14 clean.

The live API calls reported 629 uncached input tokens, 8,417 cache-write tokens, 8,417 cache-read tokens and 150 output tokens, with 83,997 request bytes and 24,840 image bytes. Individual call durations were 768–1,180 ms. These tiny transport fixtures do not measure end-to-end task performance. Credentials were loaded into the API-test subprocess and were not copied into evidence.

The native latency test measured window enumeration at 1.5 ms median, observation at 10.4 ms median, and journalled value setting at 10.6 ms median (12 samples each). These are adapter timings, not model or task completion timings.

## Release evidence that remains insufficient

The existing evaluation records are useful historical evidence, but do not establish final-release performance on this revision:

| Evidence file | Recorded commit | Strict passes |
|---|---|---:|
| `pilot-haiku.json` | `74ba6e4` | 56/57 |
| `g3-haiku.json` | `e6f2fea` | 29/33 |
| `g3-postfix-O04.json` | `ccd27c0` | 3/3 |
| `g3-postfix-X02.json` | `ccd27c0` | 2/3; one false success |

Adding the first two batches yields 85/90 = 94.44%, but this mixes revisions and is **not** a current-HEAD acceptance rate. It is also below the research's proposed 95% target. The post-fix X02 failure is a concrete task-constraint violation: Numbers wrote `~/Documents/books.xlsx`, the agent copied it into the workspace and claimed success, and the checker detected the forbidden extra file. The current per-application `file_write` grant does not enforce a destination folder. This limitation is accurately disclosed in [EVIDENCE.md](EVIDENCE.md#after-the-fixes-o04-and-x02-again-task-set-2026-11-154-hame-ccd27c0); it still matters for accepting the intended task scope.

The browser ablation covers five tasks on an older commit. Its visual arm still exposes semantic targeting; it is not the research's pure screenshot-loop baseline. It supports narrower conclusions about those configurations, not the proposed 40% time, 50% decision, 60% uncached-input and 80% image-traffic reductions. The requested network RTT/upload comparisons and broader held-out evaluation are not established by these results.

The existing source archive records commit `3698a107f233519697f93d2003b2f8d5fa7c90ca`, preceding the audited HEAD. Its checksums are correct, but it must be regenerated from the eventual release commit after fixes. The fresh local release build is not a replacement for clean-install/package validation.

This review did not rerun all 30 model-driven pilot tasks, network ablations, Linux/Windows builds, clean installation or the licensing generator. Existing evidence for those checks was inspected where relevant; it is not presented as newly measured. With A1–A3 already reproduced, another broad paid evaluation before repairs would not make this revision acceptable.

## Conditions for another acceptance review

1. Repair A1–A3 and add independent regression checks without weakening the intended predicates. Keep the current red reproduction output as historical evidence.
2. Resolve the X02 forbidden-destination behavior for the accepted task scope, and make TextEdit test startup reliable. Preserve explicit failure reporting where the task cannot be completed.
3. Run deterministic checks and the regression probes first; then run the complete supported pilot with the final code, pinned task-set version and model/settings. Report failures, false successes and forbidden effects separately. Do not combine pre-fix and post-fix attempts into a release score.
4. Measure the promised performance comparisons, or state precisely which performance claims have evidence. Continue to count cache tokens, images, retries and failed attempts.
5. Regenerate and inspect release artifacts from that same commit; run clean installation on each claimed platform and refresh the evidence/support matrix. Perform the publication decision only after the blocking findings are closed.

The positive build, API and adapter results establish substantial working functionality. They do not override the reproduced confidentiality and false-verification defects.

## Resolution (added after the review, 2026-11-15)

The decision above stands for `497fe24`. Each condition was then addressed on later commits; this section records where, without changing the review.

| Finding / condition | Change | Commit |
|---|---|---|
| A1 — file predicates bypass read confinement | In a confined session, file predicates read through the sandbox (`/bin/cat` under the worker's policy), so a link to a hidden file fails instead of returning content; regression test `command_checks_run_confined` in `hame-app` creates the link and checks the marker never appears | `6cbb13b` |
| A2 — `file_absent` on an uncheckable path | Uses `symlink_metadata`; only `NotFound` counts as absent, other errors fail the condition | `6cbb13b` |
| A3 — CSV lines instead of records | Strict record parser: quoted newlines and escaped quotes, errors on unclosed or stray quotes, every record's width must equal the header's (`csv_shape_counts_empty_quoted_and_whitespace_records` and related tests) | `6cbb13b`, `034f929` |
| 2 — X02 forbidden destination | Desktop `file_write` limited to a grant's `folders` (default the workspace), read from the save panel's "Where:" at dispatch and re-checked by the helper | `034f929`, `791c376` |
| 2 — TextEdit test startup | GUI tests retry `open` on error -600 | `6cbb13b` |
| 3 — complete pilot on final code | 30 tasks × 3 at `791c376`, release build, task set `2026-11-15.5`: 87/90, 2 failed, 1 false success; injection 11/12 (EVIDENCE.md, "Final measurement"); failures, false successes and forbidden effects reported separately | `791c376` |

The review's probe (`evidence/acceptance-2026-11-15/probe.rs`) was rerun at `791c376` with its expectations inverted: the protected marker no longer reaches model feedback, the cyclic link is not verified absent, the multiline CSV verifies for one record and not two, and the ragged CSV fails. `reproduce.py` still expects the vulnerable behaviour and therefore fails on later commits by design.

Not addressed by these commits: condition 4's performance comparisons (only the measured figures in EVIDENCE.md are claimed) and condition 5's clean installation on platforms other than macOS arm64 and Linux arm64. During the review, GUI tests ran at the same time as another session's GUI tests on the same machine; the TextEdit -600 failure and a hung Numbers process are consistent with that and are not proof of a product defect either way.
