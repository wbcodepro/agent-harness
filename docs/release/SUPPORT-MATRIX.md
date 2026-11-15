# Support matrix — hame 0.1.0-rc.1

What runs where, and what was actually tested for this release candidate. "Tested" means a run recorded in this repository (test log, evidence file or clean-install record) for this release; anything else is marked. Dates are 2026-11-14/15.

Legend: **tested** — exercised for this release; **earlier** — tested on an earlier revision, not re-run for 0.1.0-rc.1; **builds** — compiles and its unit tests pass, feature not exercised end to end; **untested**; **no** — not supported.

## Platforms × features

| Feature | macOS arm64 | macOS x86_64 | Linux x86_64 | Linux arm64 | Windows x86_64 |
|---|---|---|---|---|---|
| Build from source, `cargo test --workspace` | tested (macOS 26.6.2, Apple M5, rustc 1.99.1) | untested | untested (CI job exists, not run) | tested (Docker `rust:1-bookworm`, kernel 6.12 linuxkit, rustc 1.100.0) | earlier (Windows 11 25H2 and GitHub Actions, before this release's changes) |
| `hame` CLI, `hame run`, `hame task`, `hame doctor` | tested | untested | untested | tested (`--help`, `doctor`; no model calls) | earlier |
| Terminal interface (TUI) | tested (scripted tests) | untested | untested | builds (scripted tests pass) | earlier |
| Web interface | tested (scripted tests) | untested | untested | builds | earlier |
| ACP server | tested with the protocol's client runtime; Zed 1.28.1 and IntelliJ IDEA 2026.1 earlier (2026-11-14) | untested | untested | builds (protocol tests pass) | earlier |
| ACP / web image display | tested (unit tests); not checked in an editor | untested | untested | builds | untested |
| Shell sandbox | Seatbelt: reads, writes, network confined; tested | Seatbelt; untested | Landlock + seccomp (Linux 6.7+); untested here | Landlock + seccomp; tested (doctor probe and sandbox tests in Docker) | Low integrity: **writes only**, needs `allow_partial = true`; earlier |
| Network allowlist proxy | tested | untested | untested | tested (tests) | not enforceable (proxy only for programs that honour proxy variables) |
| Browser tool | tested (Chrome 157.0.8103.78, Playwright for Python 1.64.1, Python 3.14.5) | untested | untested | builds; browser tests skip (no Playwright in the image) | untested |
| Desktop tool | tested (macOS 26.6.2: TextEdit 1.20, Numbers 14.4, Pages 14.4, Preview 11.0) | untested | no (returns `unsupported`) | no | no |
| Outcome verification (`file_*`, `json_value`, `csv_shape`, `command`) | tested | untested | untested | tested (unit tests) | builds (`command` uses `cmd /C`) |
| `browser_state` / `desktop_state` predicates | tested (fixture tests) | untested | untested | browser: skipped; desktop: no | no |
| Workflows, `hame workflow` | tested (unit tests, CLI run) | untested | untested | tested (unit tests) | builds |
| Traces, `hame eval` arms | tested (live pilot F01–F05, see `TRACKER.md`) | untested | untested | tested (unit tests) | untested |
| WebAssembly plugins, MCP, LSP | tested (test suite) | untested | untested | tested (test suite) | earlier |

The desktop helper needs macOS 14 or later (ScreenCaptureKit); only macOS 26.6.2 was tested. Landlock needs Linux 6.7 or later for the network rules.

## Tested application and browser versions

| Component | Version | Where | Source |
|---|---|---|---|
| Google Chrome (headless, `chrome` channel) | 157.0.8103.78 | macOS 26.6.2 arm64 | [EVIDENCE-browser.md](EVIDENCE-browser.md) |
| Playwright for Python | 1.64.1 | same | same |
| Python | 3.14.5 (macOS); 3.11.2 in the Linux image (no Playwright) | | same; clean-install record in [INSTALL.md](INSTALL.md) |
| Fixture web app | `crates/hame-browser/tests/fixture/app.py` | | browser tests B01–B05 and others |
| TextEdit | 1.20 (build 415), rich and plain text | macOS 26.6.2 | [EVIDENCE-desktop.md](EVIDENCE-desktop.md), `crates/hame-desktop/README.md`; pilot D01–D05, X01, X03, R05 ([EVIDENCE.md](EVIDENCE.md)) |
| Numbers | 14.4 (7043.0.93): tables (cells read with addresses, selected, typed into; formulas through its formula editor), header rows, 2D column chart with title and axis name (Visual channel), Export To Excel | macOS 26.6.2 | pilot O01, O02, O04, X02, X04; GUI test `numbers_cells_are_observed_selected_and_typed_into` |
| Pages | 14.4 (7043.0.93): body text selected and replaced, Export To Word and PDF | macOS 26.6.2 | pilot O03, O05 |
| Preview | 11.0: opening a PDF (O05's reopen step) | macOS 26.6.2 | pilot O05 |
| Microsoft Excel, Microsoft Word | 16.78 | macOS 26.6.2 | diagnostics only, not used by any task: Excel exposes its name box, formula bar and ribbon but no cell grid (also with AXEnhancedUserInterface); a value typed after choosing a cell in the name box landed in A1. Word was not driven |
| Finder | system | macOS 26.6.2 | `windows` and menu dry resolution, manual check only |
| Swift | 6.4.2 | macOS 26.6.2 | desktop helper build |
| Zed | 1.28.1 | macOS | ACP, 2026-11-14 (earlier) |
| IntelliJ IDEA (AI Assistant) | 2026.1 | macOS | ACP, 2026-11-14 (earlier) |

**Untested**: Microsoft Edge (`msedge` channel) and Playwright's Chromium (`chromium` channel); Firefox and WebKit (not supported: the helper launches Chromium-family browsers only); the browser tool on Linux and Windows; Notes, Keynote, System Settings, Electron and Java applications; Excel and Word beyond the diagnostic above; multiple displays; non-US keyboard layouts; macOS 14 and 15.

## Providers × capabilities

| Provider / API | Text and tools | Images in tool results | Images in user messages | PDF documents | Prompt cache | Notes |
|---|---|---|---|---|---|---|
| Anthropic Messages (Claude models) | live (claude-haiku-5-5; `hame task` end to end) | live (claude-haiku-5-5) | live | fixture only | live | live tests: `HAME_LIVE_ANTHROPIC=1 cargo test -p hame-llm --test live_anthropic -- --ignored` |
| OpenAI Responses (gpt-4o, gpt-4.1, gpt-4.5, gpt-5, o-series) | earlier (evals and `hame probe`) | fixture only | fixture only | fixture only | provider-side | images as `input_image`, PDFs as `input_file` |
| OpenAI chat completions (`api = "chat-completions"`) | earlier | fixture only (sent as a follow-up user message) | fixture only | no (file parts are OpenAI's own) | provider-side | |
| DeepSeek (chat completions) | earlier (Windows run, evals) | no: text-only, images become notes | no | no | provider-side | |
| Compatible servers (OpenRouter, Gemini endpoint, Ollama, vLLM) | earlier (some) | only for names hame recognises (`gemini*`, `*-vl*`, `*vision*`, Claude, OpenAI families) or with `[model] vision = true`; fixture only | same | no | server-dependent | unknown models are text-only by default |

"fixture only" means request serialization is checked against recorded or constructed payloads in `crates/hame-llm` tests, without a live call. Costs are computed only for Anthropic models (tariff dated 2026-11-15); other providers report unknown cost.

## Known unsupported operations

- Desktop: scroll, drag, hover, choosing pop-up menu items by value, context menus; selecting a range of Numbers cells in one call (the table takes only the first cell through accessibility; `select` says so and the range is extended with shift+arrow keys); editing chart titles without the Visual channel (Numbers exposes them as text only while being edited); `click_at` on region screenshots; capturing minimized windows or windows on another Space; any platform other than macOS.
- Browser: iframes and closed shadow roots are not expanded; Firefox/WebKit; filtering a page's own subresource requests (only top-level navigation is limited to granted origins); file uploads from outside the workspace.
- Budgets: `max_cost_usd` is enforced for Anthropic models only (other providers have no tariff; such a task stops).
- MCP: requests from server to client (sampling, elicitation, roots) and background tasks.
- Sessions: no export command; no importer for Claude Code or Codex sessions (DeepSeek Harness only).
- Windows: read and network confinement of tools.
- Prebuilt binaries and installers: none for this release candidate.
