# Browser backend comparison (ADR-4)

Workstream C, 2026-11-15. Time-boxed to 30 minutes of measurement.

## Decision

**The direct Playwright helper (`crates/hame-browser`) is the primary browser backend.** agent-browser was installed and run against the same fixture. It is faster per click and per fill (it does fewer checks), but on these identical steps it silently filled nothing into a date field, which produced a wrong export, and it silently re-targeted an old ref to a re-rendered element. Both break the workstream's ref-safety and outcome requirements. Latency is not a deciding factor: every non-download operation on both backends takes well under 100 ms, against model turns of seconds.

What would reopen this: an agent-browser release that refuses refs whose element was replaced or changed (instead of re-finding by role and name), reports a fill that did not take, and exposes form-control labels for date inputs. A measured case where the extra ~50 ms per click matters.

## Method

- Machine: Apple M5, macOS 26.6.2, Google Chrome 157.0.8103.78 (system install, headless), Node v24.22.3, Python 3.14.5, Playwright for Python 1.64.1, rustc 1.99.1.
- Fixture: `crates/hame-browser/tests/fixture/app.py` (stdlib HTTP server on 127.0.0.1, random port), the same one the integration tests use.
- Identical steps, 12 repetitions each (10 for downloads): load `/form`, snapshot/observe it, fill the Shipping "Name" field by ref, click the "Express delivery" checkbox by ref; then on `/export` fill a date range and download the delayed (1.6 s) CSV by clicking "Export CSV".
- hame: `cargo run --release -p hame-browser --example latency -- 12`. Times are around a full `Tool::call` (JSON in, model-facing text out), in process, with the helper already running.
- agent-browser 0.41.0 (published 2026-10-29), installed with `npm install agent-browser@0.41.0` into a scratch directory; native `agent-browser-darwin-arm64` CLI. Run with `crates/hame-browser/bench/agent_browser.py 12` (headless, `--executable-path` to the same Chrome, a temporary HOME and `--profile` directory, a short `AGENT_BROWSER_SOCKET_DIR`). Times are around each CLI invocation, which includes spawning the CLI process (a few ms) and talking to its already-running daemon. That is how a harness would call it as a CLI; its MCP mode was not measured.
- Median and p95 are taken over the raw samples below (p95 = the ceil(0.95 n)-th sample).

## Results

Startup (first open, including helper or daemon and Chrome launch): hame 568 ms, agent-browser 443–525 ms.

| Step | hame median | hame p95 | agent-browser median | agent-browser p95 |
|---|---:|---:|---:|---:|
| Load page | 21.3 ms (navigate **+ observation**) | 22.9 | 10.9 ms (open only) | 24.9 |
| Snapshot / observe | 5.2 | 6.3 | 6.5 | 10.4 |
| Fill by ref | 11.1 | 18.3 | 5.0 | 148.1 |
| Click by ref (non-navigating) | 62.5 | 69.0 | 11.0 | 12.9 |
| Download (1.6 s server delay) | 1698.8 | 1710.7 | 1641.9 | 1661.6 |

Raw samples (ms):

- hame navigate+observe: 20.4, 20.9, 21.1, 21.3, 21.3, 21.3, 21.3, 21.3, 21.4, 21.5, 21.7, 22.9
- hame observe: 5.0, 5.1, 5.1, 5.1, 5.1, 5.1, 5.2, 5.2, 5.5, 5.6, 5.9, 6.3
- hame fill: 10.9, 11.1, 11.1, 11.1, 11.1, 11.1, 11.1, 11.2, 11.3, 11.5, 11.5, 18.3
- hame click: 61.1, 61.1, 61.7, 62.2, 62.3, 62.3, 62.5, 62.6, 62.6, 63.2, 64.1, 69.0
- hame download: 1696.6, 1697.3, 1698.3, 1698.4, 1698.5, 1698.8, 1699.3, 1704.1, 1705.0, 1710.7
- agent-browser open: 9.7, 10.0, 10.0, 10.4, 10.6, 10.8, 10.9, 10.9, 11.5, 11.9, 13.0, 24.9
- agent-browser snapshot: 5.8, 6.1, 6.3, 6.4, 6.4, 6.5, 6.5, 6.6, 6.6, 6.6, 6.8, 10.4
- agent-browser fill: 4.8, 4.8, 4.8, 4.8, 4.9, 4.9, 5.0, 5.0, 5.0, 5.0, 5.0, 148.1
- agent-browser click: 7.4, 9.6, 9.6, 10.0, 10.4, 10.4, 11.0, 11.2, 11.3, 11.8, 11.9, 12.9
- agent-browser download: 1630.3, 1630.8, 1631.2, 1635.9, 1641.8, 1641.9, 1645.9, 1649.3, 1651.9, 1661.6

Why hame is slower where it is: a click is an inspect round trip (what the element is decides Commit versus Interact, and it is authorized and journalled before dispatch) plus a Playwright trial click as the actionability preflight, then the click itself, which waits for the element to be stable again; each stability check waits for animation frames. A fill is two round trips plus a read-back of the value. A page load returns an observation, so it is one call where agent-browser needs two (10.9 + 6.5 ms).

## Behaviour on the same steps

| Check | hame | agent-browser 0.41.0 |
|---|---|---|
| Duplicate labels ("Name" under Shipping and Billing) | distinct refs, each shown with `(in Shipping)` / `(in Billing)`; text targeting is refused as `ambiguous` with both candidates | distinct refs, but the interactive snapshot gives no way to tell which "Name" is which |
| Date inputs | `textbox "From"`, `textbox "To"` with `type=date` | only unlabeled internal parts: `spinbutton "Day Day"`, `"Month Month"`, `"Year Year"` |
| `fill` of a date input (`2026-01-10`) | value set and read back | reports `✓ Done`, value stays empty; the export then contained all 90 ledger rows instead of 11 |
| Old ref after the list is re-rendered (several equal "Edit" buttons) | `stale_ref` (element removed) | refused: "cannot be recovered: the snapshot had several matching elements" |
| Old ref after a unique element is re-rendered | `stale_ref` | **clicked the new element** (`✓ Done`; the new node's handler ran) |
| Old ref after its element's label changed in place ("Edit" → "Modify") | `stale_ref` (name changed) | **clicked it** (`✓ Done`) |
| Old ref after navigation | `stale_ref` | refused ("Unknown ref") |
| Native `confirm` during an action | `dialog_open`, a batch stops with its completed prefix | reported as a warning after `✓ Done`; the next command fails until it is resolved |
| Delayed download | completes, saved in the workspace, stored as an artifact | completes |

## Not measured

agent-browser's MCP server mode, its own `batch` command, its `--allowed-domains` and `--action-policy` features (hame enforces authorization itself, before dispatch, and blocks navigation in the helper), Linux and Windows.
