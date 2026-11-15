# hame

English · [Tiếng Việt](README.vi.md)

`hame` is an agent harness written in Rust: one binary that runs a language model as an agent in your terminal, in your editor (over ACP) or in a browser. It began as a coding agent. Version 0.1 adds tasks whose outcome hame checks itself, and tools that operate a web browser and native macOS applications.

How a task runs: **outcome → tools → verify → evidence**.

1. **Outcome.** You state what must be true when the work is done (a task file: the outcome, acceptance conditions, the scope you grant, a budget).
2. **Tools.** The agent works with files and the shell, and, where you granted it, a browser and desktop applications. It is told to prefer files, commands and APIs, then structured page and accessibility data, and screenshots last.
3. **Verify.** hame checks the acceptance conditions against the real state (files, JSON, CSV, command exit codes, the open page, an application's window), not against the agent's report. Failing conditions send the agent back, at most twice.
4. **Evidence.** The verification report, a receipt for every action on an application, the session log, stored screenshots and downloads, and optionally a trace of every model call and tool call. `hame task` exits 0 only when the outcome is verified.

Three ideas shape the design:

- **The log is the source of truth.** Each session is an append-only JSONL file. Everything the model sees (system prompt, tool list, messages) is rebuilt from the log, so resume, fork, rewind, context compaction and replay all share one mechanism.
- **Tools do not run inside the harness.** Every tool call runs in a worker process confined by the operating system (Seatbelt on macOS, Landlock on Linux). If hame cannot confine it, it refuses to run it.
- **Extensions without extra privileges.** Plugins are WebAssembly and can read only what you grant; MCP servers run in the same sandbox as tools.

What sets it apart from other harnesses:

- **"Done" needs evidence.** Before a turn that changed code may end, the project's check commands must pass. If the model tries to stop with a failing test, or after changing code without running anything, it is sent back (`stop_on_red`). An optional independent reviewer can look for rules the change missed.
- **It escalates when stuck.** After repeated failing checks or repeated calls, hame raises the reasoning effort or hands the rest of the turn to a stronger model, possibly from another provider.
- **Edits are checked as they land.** A syntax check runs after every edit, LSP diagnostics are reported, and callers and tests of a changed signature are named.
- **Secrets stay out of the model's reach.** Tools get stand-in values, and the egress proxy puts the real secret into requests to the hosts you name. Network access is limited to an allowlist.
- **Cost-aware context.** The prompt keeps a stable, cached prefix. Passing tests and build progress are dropped from command output before they are logged. When the summary model is the main model, compaction reuses the conversation's cached prefix.
- **Built-in evaluation.** `hame eval` runs task suites with hidden checks, and can run other agents on the same tasks.

## Install

hame is built from source. It needs Rust 1.89 or later, and `git`. The browser tool also needs Python 3 with Playwright and Google Chrome; the desktop tool needs macOS 14 or later and the Swift compiler. [docs/release/INSTALL.md](docs/release/INSTALL.md) has the steps per platform and feature, and [docs/release/SUPPORT-MATRIX.md](docs/release/SUPPORT-MATRIX.md) what was tested where.

```sh
cargo install --git https://github.com/wbcodepro/agent-harness hame-cli
# or, from a clone:
cargo install --locked --path crates/hame-cli   # installs ~/.cargo/bin/hame
cargo build --release -p hame-cli               # or just build target/release/hame
```

Then run `hame setup`. It asks which provider to use and for an API key; the key is not shown on screen and is saved in `~/.hame/.env`, readable only by you. It then checks the key, the sandbox, `bash` and `git`, and can send one test request that costs a few tokens. `hame doctor` runs the same checks at any time (`--ping` sends the test request).

You can also put the key in the environment or in `~/.hame/.env` (or `.env` in the working directory) yourself:

```sh
DEEPSEEK_API_KEY=...     # or ANTHROPIC_API_KEY / CLAUDE_API_KEY, OPENAI_API_KEY
```

| Provider | Key variable | Default model |
|---|---|---|
| `deepseek` | `DEEPSEEK_API_KEY` | `deepseek-flash` |
| `anthropic` | `ANTHROPIC_API_KEY`, or `CLAUDE_API_KEY` | `claude-opus-5-5` |
| `openai` | `OPENAI_API_KEY` | none: pass `--model` or set `[model] name` |
| a compatible server (`provider = "openai"` with `base_url`) | `HAME_API_KEY` | none |

`[model] api_key_env = "NAME"` reads the key from another variable. Without `--provider`, `HAME_PROVIDER` or `[model] provider`, hame takes the first of DeepSeek, Anthropic and OpenAI whose key it finds. The data directory is `~/.hame`; `--home` or `HAME_HOME` moves it.

## Use

```sh
hame setup                            # choose a provider, save its key, check everything
hame doctor [--ping]                  # check configuration, key, sandbox, programs, browser and desktop
hame                                  # terminal interface
hame run "fix the bug that makes test_stats.py fail"   # one turn, non-interactive
hame task task.toml [--json]          # work until the task file's conditions hold; exit 0 only when verified
hame run --trace session "..."        # also write a trace of model and tool calls beside the session log
hame workflow list | show <name> | trust <name> | untrust <name>   # saved workflows and their trust
hame run --session <id> "carry on"    # continue a session
hame run --plan "..."                 # investigate and propose a plan; change nothing
hame run --output json "..."          # one JSON object when done (stream-json: also one event per line)
hame web                              # web interface on 127.0.0.1
hame acp                              # Agent Client Protocol server for editors
hame sessions | show <id> | fork <id> | rewind <id> | recover <id>
hame sessions gc [--older-than-hours 24] [--dry-run]   # delete stored images no session refers to
hame import dsh <session.jsonl>       # import a DeepSeek Harness session (v2–v4)
hame eval evals/harder                # run a task suite; report pass rate, time and tokens
                                      # (--keep <dir>: keep logs of failed tasks; --runs N: N runs, per-task pass rate)
hame eval evals/harder --command 'codex exec "$HAME_EVAL_PROMPT"'   # measure another agent on the same suite
hame eval evals/expert                # ten harder tasks with hidden checks (Python, Rust, Go, Node)
hame skill install <dir | git URL> [--path sub/dir]    # scan, then install a skill into ~/.hame/skills
hame skill list
hame plugin install <dir | git URL> [--path sub/dir]   # scan, then install a Claude Code style plugin into ~/.hame/plugins
hame plugin install <marketplace> --plugin <name>      # install one plugin from a marketplace catalogue
hame plugin update [name]                              # fetch again, rescan, replace the installed copy
hame plugin list
hame mcp login <server> | logout <server>   # OAuth sign-in to an MCP server over HTTP
hame probe evals/harder --model <name> # try a model with each edit format and tool set; print the best configuration
```

Choose the model with `--provider deepseek|openai|anthropic` and `--model <name>`, or `HAME_PROVIDER` / `HAME_MODEL`. Defaults: `deepseek-flash` with DeepSeek, `claude-opus-5-5` with Anthropic; OpenAI needs a model name. Any server that speaks OpenAI's chat completions (OpenRouter, Gemini's OpenAI endpoint, Ollama, vLLM) works with `provider = "openai"`, `base_url` and `api = "chat-completions"`; `hame setup` writes this for you when you pick "another server".

At the end of each `hame run`, a summary line goes to stderr: tokens sent, the share read from the provider's cache, tokens received and the number of requests. hame warns when most of the prompt was not reused between requests.

In the terminal interface: `/plan`, `/rewind`, `/new`, `/quit`; Esc stops the running turn. Typing a message and pressing Enter while a turn runs sends it to that turn: the model gets it with its next request instead of after the turn (over ACP this is the `_hame/steer` extension notification).

### In an editor (ACP)

`hame acp` speaks the Agent Client Protocol over stdin/stdout.

**IntelliJ IDEA** (AI Assistant): create `~/.jetbrains/acp.json` declaring the agent, restart the IDE and choose `hame (local ACP)` in AI Chat:

```json
{ "command": "/path/to/hame", "args": ["acp"], "env": {} }
```

Tested in IntelliJ IDEA 2026.1 on 2026-11-14; all five checks passed: the answer streams in and tool calls show as their own items; creating a file; stopping midway and asking again; reopening an earlier conversation; the IDE asking for permission before a shell command in `danger-full-access` mode.

**Zed:** add this to `~/.config/zed/settings.json`, then choose `hame` in the External Agents menu:

```json
{ "agent_servers": { "hame": { "type": "custom", "command": "/path/to/hame", "args": ["acp"] } } }
```

Tested in Zed 1.28.1 on 2026-11-14 with the same five checks, all passed (stopping midway was tested with Esc; the Stop button was not confirmed). Without `"type": "custom"` Zed does not pick up the agent.

The server is also tested automatically with the protocol's official client runtime (`agent-client-protocol`), including the permission flow.

## Tasks with a verified outcome

`hame task FILE` runs one task described in a TOML file, then checks its acceptance conditions itself:

```toml
# task.toml
id = "ledger-summary"                       # letters, digits, - and _
outcome = "Export the ledger for 10-20 January 2026 from the local app and write the total to out/summary.txt"
constraints = ["Use the browser profile 'work'", "Do not change any other file"]

[budget]
max_steps = 40              # model requests
max_tokens = 400000         # input + output tokens of the agent and its subagents
deadline_ms = 600000        # wall-clock time

[[acceptance]]              # every condition must hold
kind = "csv_shape"
path = "out/ledger.csv"
rows = 11
columns = ["date", "description", "amount"]

[[acceptance]]
kind = "file_contains"
path = "out/summary.txt"
text = "Total: 1,234.50"

[[authorization.grants]]    # what the agent may do, and where
channel = "browser"
origin = "http://127.0.0.1:*"
profile = "work"
capabilities = ["observe", "interact", "file_write"]
```

```sh
hame task task.toml                          # progress on stderr; the verdict and evidence at the end
hame task task.toml --json                   # one JSON object: status, verified, each condition with its evidence, receipts, tokens, timings
hame task task.toml --trace session          # also write trace.v1.jsonl beside the session log
hame task task.toml --cwd ~/work/project --provider anthropic --model claude-haiku-5-5 --yes
```

Acceptance conditions (`kind`): `file_exists`, `file_absent` (`path`); `file_contains` (`path`, `text`, exact and Unicode-preserving); `file_sha256` (`path`, `sha256`); `json_value` (`path`, `pointer` as RFC 6901, `equals`); `csv_shape` (`path`, `rows` without the header, `columns`); `command` (`command`, `exit`, default 0; run with `sh -c` in the working directory under the same sandbox as the agent's tools, with the network off, only the basic environment variables (no credentials) and a 120-second limit: the agent can change the files such a command runs, such as a test or a Makefile, so the check gets no more access than the agent; with `--sandbox danger-full-access` it runs unconfined, like everything else); `browser_state` (`url_contains`, `text_contains`, `profile`; reads the live page); `desktop_state` (`app` bundle id, `window_title_contains`, `value_contains`; reads the live application). Paths are relative to the working directory. A task without acceptance conditions is refused: its completion could not be verified. So is a condition that would check nothing (an empty `text`, `command` or `path`; a `browser_state` without `url_contains` or `text_contains`; a `desktop_state` without `window_title_contains` or `value_contains`) and any misspelt or unknown field, in conditions, grants and the budget: `text_contians` is an error, not a condition that always passes, and `orgin` is an error, not a grant for every origin. A `browser_state` with `profile` fails when the browser cannot confirm the profile. Workflow parameters substituted into a `command` condition are shell-quoted, one word each.

The agent is told the conditions so it can aim at them; hame evaluates them afterwards against the real state. When the agent ends its turn with a condition failing, it is sent back with the failing evidence, at most twice. `hame task` exits 0 when every condition holds, 1 when one does not (or the turn failed), 130 when interrupted. The log records `task/start` (the contract) and `task/verified` (the report), and the action receipts. `max_steps`, `max_tokens`, `deadline_ms` and `max_cost_usd` are enforced before each model request. `max_tokens` counts the subagents the task starts too (it caps the shared `[limits] tree_tokens` budget). The cost is priced from a dated tariff that covers Anthropic models only; with a cost budget set and a model whose price is unknown, the task stops rather than run unmetered. Subagent spending is counted in tokens but not priced, so a task with `max_cost_usd` is not offered the subagent tool. Requests that fail without the provider reporting usage are not counted.

The grants in a task file add to those in `config.toml` for that run only, and turn on the browser or desktop tool when they name its channel.

## Browser and desktop control

Two host-side tools act outside the shell sandbox: `browser` (Chrome through Playwright; tested on macOS arm64 only, see [SUPPORT-MATRIX](docs/release/SUPPORT-MATRIX.md)) and `desktop` (native macOS applications through the accessibility interface). Both are off until the configuration turns them on or a grant names their channel, and each action is checked against the grants before it is sent.

```toml
[computer]
browser = true                  # offer the browser tool
desktop = true                  # offer the desktop tool (macOS 14+)
headless = true                 # browser without a window (default)
browser_channel = "chrome"      # chrome (installed Google Chrome, default) | msedge | chromium
# batch = true                  # the tools' batch action (default on; off only to measure it)
# observation = "semantic"      # semantic (default) | screenshot | pixels (both need a "screen" grant; screenshot measures vision cost, pixels is the screenshot-loop baseline: screenshots only, clicks by coordinates)
# desktop_background = false     # true: the desktop tool works without taking your screen: text, plain keys, and new_document / open_file / save through Apple Events; menus, pixel clicks and shortcuts are refused
# profiles_dir = "/path"        # persistent browser profiles; default ~/.hame/browser-profiles

[[computer.grant]]              # read and use pages on local servers, profile "work" only
channel = "browser"
origin = "http://127.0.0.1:*"
profile = "work"
capabilities = ["observe", "interact"]

[[computer.grant]]              # TextEdit: read, press, type, save files through its save panel
channel = "desktop"
app = "com.apple.TextEdit"
capabilities = ["observe", "interact", "global_input", "commit", "file_write"]
folders = ["~/Documents/reports"]   # where its save panel may write (default: the workspace)

[[computer.grant]]              # TextEdit screenshots and clicks on screenshot pixels (not on Save, Delete, ...)
channel = "visual"
app = "com.apple.TextEdit"
capabilities = ["screen", "global_input"]
```

A grant has a `channel` (`browser`, `desktop`, or `visual` for screenshots and pixel clicks of native windows), optional `app` (bundle id), `origin` and `profile` patterns (`*` matches any run of characters, case-insensitive; an absent pattern matches anything), optional `folders` (desktop `file_write`: the folders a save or export panel may write into, each with its direct subfolders; absolute, `~/` or relative to the workspace; `"*"` for any folder; default the workspace only), and `capabilities`:

| Capability | Allows |
|---|---|
| `observe` | read pages, windows and accessibility trees; wait; extract |
| `interact` | click, fill, select, press, set values, menus that only open something |
| `commit` | actions that submit, send, save, delete, pay or purchase (by control name, form method, or `commit: true` from the model), by whatever route: on the desktop also Return or Space where it presses a button, cmd+s, cmd+w, cmd+q, cmd+d, cmd+delete, a newline typed anywhere but the text area of a document editor (TextEdit, Pages, Numbers, Keynote, Notes, Word; in any other app, such as Messages or Slack, Return may send), and a pixel click on such a control or on anything hame cannot identify |
| `file_write` | downloads into the workspace (see below); confirming a save or export panel (its Save or Export button, Return or a typed newline in it, a click on Save) and cmd+s. On the desktop, only into the grant's `folders` (default the workspace): hame reads the panel's "Where:" pop-up (folder and parent names) when it is confirmed and refuses any other folder, or a panel whose folder it cannot read (an expanded panel). Saving an already saved document in place (cmd+s, File > Save) writes where that document is |
| `clipboard` | read and set the clipboard (desktop; the previous contents are restored), and cmd+c, cmd+x, cmd+v |
| `global_input` | keyboard events, key combinations and pixel clicks; needs the target frontmost and holds a cross-process lease. Alone it covers typing and keys that do not commit; see `commit` |
| `screen` | screenshots (stored as artifacts) |

Security model:

- **Grants come only from you**: `[computer]` in `~/.hame/config.toml`, or a task file you pass to `hame task`. A project's `.hame/config.toml` cannot grant them (it may only set `[verify]`). Text in a page, a document, an accessibility label or a screenshot is marked untrusted, and the agent is told it can never widen its scope. A refused action says which capability on which target was missing.
- **Desktop and browser authority is separate from the shell sandbox and is not confined by it.** The helpers run as your user, with your terminal's macOS privacy permissions (Accessibility, Screen Recording) and your browser profiles' cookies. Seatbelt, Landlock and the network allowlist apply to `bash` and other confined tools, not to these. Grant narrowly: an app or origin pattern, and only the capabilities the task needs.
- The browser helper blocks top-level navigation to origins no grant covers, before the request leaves the machine. Each redirect is checked twice: its `Location` is resolved the way Chrome resolves it (a backslash counts as a slash, so `/\evil.example/` leads to evil.example; a target it cannot resolve with certainty is refused), and the request Chrome then makes is checked at the URL Chrome itself computed. A page's own requests (scripts, images, API calls) are not filtered, and the browser does not go through hame's proxy.
- Downloads are saved only inside the workspace, and never through a symbolic link: a destination that is a link (wherever it points), or a directory path that leads outside through one, is refused. An existing file is replaced only when the call passes `overwrite: true`, and then only a regular file; the download is written to a new temporary file and renamed into place, so it never writes into an existing file, a link or another hard link of it. When the path is a directory, the page's suggested name is reduced to a plain file name (no separators, `..`, leading dots, control characters or Windows-reserved names; at most 120 bytes). An upload reads only a regular file inside the workspace, not through a link and not one with other hard links, and the browser is given a private copy.
- Every acting operation is journalled (`not_started`, `dispatched`, then `completed`, `failed` or `outcome_unknown`) before and after it is sent. A commit carries an idempotency key; one that may already have taken effect is not repeated until the page or window has been observed again and the agent names the earlier operation (`reconciled`). After a crash, dispatched actions become `outcome_unknown` and the agent is told to check the state; hame never replays them.
- Refs (`e12`, `o3:e7`) belong to one observation. A ref whose page navigated, whose element changed or was removed, or whose window closed is refused (`stale_ref`, `stale_window`), and two matching targets are refused as `ambiguous`; the tools never re-target a "similar" element. Physical input stops when focus moves (`interference`) or a dialog appears (`unexpected_dialog`) and reports exactly what was typed.
- Commit detection is a heuristic over control names and form methods: a button that posts through JavaScript may count as `interact`. Do not rely on withholding `commit` as the only safeguard on a site where a click can do damage. On the desktop, physical input is classified from where keyboard focus is (or what is under a clicked pixel), read from the application right before the input is sent; the helper refuses the input if that changed in between, and when it cannot be read the stricter set is required. Typing ordinary text into a field, and Return in a document's text area, need only `global_input`.

Browser profiles are persistent directories (`~/.hame/browser-profiles/<name>`), so a profile you sign in to stays signed in; treat it like a browser profile of your own. Downloads go into the workspace, under the rules above. The desktop helper is a Swift program built on first use into `~/.hame/helpers/` (about 4 seconds). Details per tool: `crates/hame-browser`, [crates/hame-desktop/README.md](crates/hame-desktop/README.md).

## Workflows

A workflow is a saved, parameterized sequence of tool calls with checks before and after, for a job that has been done before. Files live in `.hame/workflows/NAME.toml` in the project, or `~/.hame/workflows/NAME.toml`; the project's copy wins. When any exist, the agent gets a `workflow` tool listing them.

```toml
# .hame/workflows/export-ledger.toml
name = "export-ledger"
version = 1
description = "Export the ledger for a date range as CSV"
provenance = "written by hand after a manual run"
tested_with = ["fixture app", "Chrome 157"]

[params.from]
pattern = '\d{4}-\d{2}-\d{2}'   # the whole value must match
[params.to]
pattern = '\d{4}-\d{2}-\d{2}'

[[requires]]                     # grants it needs, checked before it runs
channel = "browser"
capability = "file_write"

[[pre]]                          # checked against the live state first; failure = drift, nothing runs
kind = "browser_state"
url_contains = "/export"

[[steps]]
tool = "browser"
input = { action = "fill", text = "From", value = "{{from}}" }
[[steps]]
tool = "browser"
input = { action = "fill", text = "To", value = "{{to}}" }
[[steps]]
tool = "browser"
input = { action = "download", text = "Export CSV", path = "out/ledger.csv", overwrite = true }
note = "the server takes a second or two"

[[post]]                         # at least one; the run fails if these do not hold
kind = "csv_shape"
path = "out/ledger.csv"
columns = ["date", "description", "amount"]
```

A workflow runs only if you trusted that exact file: `hame workflow show NAME` prints what it needs, checks and does, and `hame workflow trust NAME` records its SHA-256 in `~/.hame/trusted-workflows.json` after you confirm. Editing the file revokes the trust; trust cannot be given from inside a session (the agent's tools cannot write hame's data directory). Each step still goes through permission rules, grants, approval and the action journal like a call the model made, and steps address elements by role, label or selector, never by recorded coordinates or refs. A run stops at the first failed step and reports the completed prefix; parameters that do not match their pattern are refused before anything runs.

## Built-in tools

`read`, `write`, `edit`, `bash`, `job_output`, `glob`, `grep`, `repo_map` (also `symbol`: where a name is defined, what calls it, what it calls), `subagent`, `task_create` / `task_update` / `task_list` / `task_claim` (a board shared with subagents: dependencies, owners, claiming a free task); `send_message` (to the lead or a running subagent, by name); `skill` when there are skills; `diagnostics`, `references` and `rename_symbol` when a language server is configured; `browser` and `desktop` when [enabled or granted](#browser-and-desktop-control); `workflow` when [workflows](#workflows) exist. With `edit_format = "apply-patch"`, `apply_patch` replaces `edit` and `write`.

`write` refuses to overwrite an existing file the model has not read in the session (a file's content is usually what the task depends on); when a write shrinks a file by more than half, the result says the whole file was replaced. When `edit` cannot find the text to replace, it shows how far it matched and where it differs. `grep` names the function or class around each match. After every `edit`, `write` or `apply_patch`, if a valid source file has become syntactically invalid, the result points at the error (Python is checked with `python3` when present; Rust, JS, TS and Go with tree-sitter).

A `bash` command still running at `timeout_ms` (120 seconds by default) is not killed: it becomes a background job, still in the sandbox. The result returns the output so far and a job id; the model calls `job_output` to wait and read the rest, or stops the job with `kill: true`. A long build or test run therefore does not start over. Jobs end when hame exits, or after an hour. A test command still running in the background does not count as passing: the completion check tells the model to wait for its result before finishing.

Model calls: when the provider sends `Retry-After`, hame waits that long (up to 60 seconds; a longer wait is reported at once instead of waited out in silence). When a request is refused because the prompt exceeds the context window, hame compacts the history at once (keeping about the last quarter verbatim) and sends it again, once.

`references` and `rename_symbol` ask the language server directly instead of a chain of `grep` and `edit`: the model names a symbol by file, line and name (no column counting). `references` lists every use as `path:line: line text`, up to 100 lines. `rename_symbol` renames across all files in one call and reports the edits per file; every edit is computed before anything is written, and if any edit falls outside the working directory, or the server asks to rename, create or delete files, no file is changed. When there is no server for the file type, or the server lacks the feature, the result tells the model to fall back to `grep` and `edit`.

`edit` and `apply_patch` tolerate differences in whitespace, indentation and line endings when there is exactly one match; unchanged lines are kept exactly as they are in the file. Tool arguments wrapped in a code fence or mixed with prose are unwrapped before running.

When MCP servers and plugins provide more tools than `defer_tools_over`, those tools are left out of the tool list sent to the model (each declared tool costs tokens on every request). The model finds them with `tool_search` and runs them with `tool_call`; permission rules and hooks still see the real tool name. In that case there is also a `script` tool: the model writes a short Python program that calls those tools as functions (`call(name, **args)`), so a chain of calls costs one step and only what the program prints enters the context. The program runs in the sandbox like a shell command; each call inside it still goes through permission rules and hooks; the program gets each call's full result, and only what it prints is subject to `spill_at_bytes`. Needs `python3` (or `python`); turn it off with `[context] scripts = false`.

Read-only calls the model issues together run in parallel. `subagent` hands a task to a new agent with its own context; with `isolate` it works on a separate git worktree and branch.

## Safety

By default (`workspace-write`):

- Tools can write only in the working directory and the temp directories.
- They cannot read `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.config/gh`..., hame's own data, or `.env` files.
- The network goes only through a local proxy, to an allowlist of domains.
- The harness's environment variables do not reach tools; a secret reaches them only when you grant it (`[secrets] grant`), and the value of every secret is replaced by `[secret:NAME]` in tool output before it is logged.
- Before each turn and after each step that changed something, the workspace is saved to a separate git store outside the project; `hame rewind` restores the files and starts a new session that ends just before that turn.
- Changes a tool call makes to `.git/hooks` and `.git/config` of the repository in the working directory are undone right after that call, and the model is told (`[sandbox] protect_git = false` turns this off). This is detection and undo, not prevention: a hook exists while the command runs. A repository the call itself created (`git init`) is not protected.
- Permission rules for `bash` are matched against each command in the line (the parts separated by `;`, `&&`, `||`, `|`, the contents of `$(...)` and backticks, the command after `sudo`/`env`...): a `deny`/`ask` rule applies wherever the command appears, and an `allow` rule applies only when every command in the line is allowed. The splitter is not a full shell parser.
- With `[approval] reviewer = true`, a second model looks at each call that needs permission (it sees only the tool name, arguments and working directory, not the conversation) and approves on its own only calls that are clearly harmless; everything else, including unclear answers, is still asked of you. Every verdict is logged to `~/.hame/approvals.jsonl`. It is a model and can be wrong: it reduces how often you are asked; it does not replace the sandbox.
- With `[secrets] substitute`, a secret never enters the sandbox: the tool's environment holds a stand-in, and the network proxy replaces it with the real secret in the request head (request line and headers, including `Authorization: Basic`) sent to the hosts you name. For HTTPS the proxy must read the request, so it terminates the tool's TLS connection for those hosts only, with a root certificate generated for this run, and opens its own verified TLS connection to the real host; other hosts are passed through untouched. Tools are pointed at that root certificate through `SSL_CERT_FILE`, `CURL_CA_BUNDLE`, `REQUESTS_CA_BUNDLE`, `GIT_SSL_CAINFO`, `PIP_CERT`, `CARGO_HTTP_CAINFO` and `NODE_EXTRA_CA_CERTS`; programs that read none of them refuse to connect to that host. A request body is also substituted when it declares `Content-Length` and is at most 1 MiB (the length is corrected). Limits: chunked or larger bodies pass through untouched; over plain HTTP only the head is substituted; HTTP/1.1 only; one request per connection; a host you name that echoes headers back would return the secret in output (where it is masked like every secret). The certificate list given to tools is the operating system's store plus the proxy's certificate.

`--sandbox read-only` forbids writes; `--sandbox danger-full-access` removes confinement, and then every tool that is not read-only needs your approval, except `browser` and `desktop` (below).

The `browser` and `desktop` tools are the exception to "tools do not run inside the harness": they act on your browser profiles and applications from outside the sandbox, under the grants you give, as described in [Browser and desktop control](#browser-and-desktop-control). They are off unless configured or granted. With `[approval] default = "ask"` (the default) their calls are not asked about one by one, because the grants already decide what each action may do; with `default = "deny"` or `"allow"` they follow that default like any other tool, and an `[[approval.rule]]` for `browser` or `desktop` still applies.

Known limits:

- The temp directories are writable by default (`[sandbox] writable_tmp = false` turns that off). A project inside `/tmp` can write to its sibling directories.
- On Linux, Landlock cannot hide files by name: a `.env` inside the workspace stays readable (its values are still masked in the log); the parent directory of a hidden path cannot be listed; the network is limited by TCP port, and UDP is not blocked. Needs Linux 6.7 or later.
- On Windows only **writes** are confined: the process running tools is lowered to Low integrity, so it can write only in the project directory and a private temp directory (`AppData\LocalLow\hame-tmp`). Tools can still read every file you can and reach the network, so this must be accepted explicitly with `allow_partial = true` under `[sandbox]` (`hame setup` asks); otherwise the confined modes refuse to start. The project directory is labelled Low permanently (other Low programs of the same user can write to it too). `AppData\LocalLow` is writable as well, since Windows labels it Low. Files or directories **moved** (not copied) into the project after the first run keep their old label and cannot be changed by tools; relabel with `icacls <project dir> /setintegritylevel "(OI)(CI)L" /T`. The `bash` tool uses Git for Windows' bash, found through `git` on PATH. That bash keeps state shared per installation: while another Git Bash is open outside the sandbox, bash inside it cannot start (the tool says so); close it, or set `HAME_BASH` to the `bash.exe` of a second Git installation used only by hame. The network proxy cannot be enforced on Windows; it starts only with `[secrets] substitute`, and only programs that honour the proxy variables use it (Python, Node, git; the curl shipped with Git for Windows rejects the proxy's certificate). A program that bypasses the proxy can reach any host but only holds the stand-in. Tested on GitHub Actions and on a real Windows 11 machine; results and what is untested are in `docs/WINDOWS-RESULTS.md`.
- This software has not had a security audit. For untrusted code, run it in a virtual machine or container. To report a vulnerability, see [SECURITY.md](SECURITY.md).

## Configuration

`~/.hame/config.toml`. Every entry is optional; unknown keys are errors.

```toml
[model]
provider = "anthropic"          # deepseek | openai | anthropic
name = "claude-opus-5-5"
effort = "high"                 # reasoning effort (Anthropic, OpenAI)
api = "responses"               # with openai: responses (default) | chat-completions
base_url = "https://openrouter.ai/api/v1"   # a compatible server instead of the provider's own
summary_model = "claude-haiku-5-5"   # a cheaper model for summarizing history
tools = "full"                  # full | minimal (files, search and shell only; for small models)
edit_format = "search-replace"  # search-replace | whole-file | apply-patch | hashline (read tags each line with a short
                                # hash; edit_lines edits by those tags and refuses when the file has changed)
max_tool_calls = 32             # most tool calls in one response
fuse_actions = true             # edit/edit_lines/write/apply_patch take then_run: a command run right after the
                                # change, its result returned with the same call
temperature = 0.0               # chat-completions API only (DeepSeek, compatible servers, local models)
escalate_to = "claude-fable-5-2"     # a stronger model takes over the rest of the turn when the main one is stuck
escalate_provider = "anthropic"      # optional: the stronger model is at another provider (e.g. the main one is local)
escalate_effort = "high"             # without escalate_to: same model, more reasoning when the turn is stuck,
                                     # so everyday effort can be set low
escalate_after = 2              # after this many failing checks or repeated calls in a turn
# vision = true                 # the model accepts images (default: known Claude/OpenAI models do, others are text-only)
# pdf = true                    # the model accepts PDF documents

[sandbox]
mode = "workspace-write"        # read-only | workspace-write | danger-full-access
allow_partial = false           # Windows: accept confining writes only
protect_git = true              # undo tool changes to .git/hooks and .git/config
writable = ["/data/cache"]
unreadable = ["/data/private"]

[network]
mode = "allowlist"              # off | allowlist | open
allow = ["github.com", "*.crates.io"]

[[rule]]                        # the first matching rule wins
tool = "bash"
pattern = "git push*"
action = "ask"                  # allow | ask | deny

[approval]
default = "ask"                 # for calls no rule matches: allow | ask | deny
reviewer = false                # true: a model (summary_model) answers permission questions first;
                                # it can only approve clearly harmless commands, the rest still ask you

[secrets]
grant = ["GITHUB_TOKEN"]        # give a secret to tools as is
substitute = { NPM_TOKEN = ["registry.npmjs.org"] }   # tools get a stand-in; the proxy puts the real secret
                                # into requests to these hosts (needs [network] mode = "allowlist")

[context]
compact_at_tokens = 100000      # summarize older history past this size
clear_tool_results = false      # true: past the threshold, clear old tool results first, summarize only if not enough
spill_at_bytes = 16000          # longer output is saved to a file and only previewed
project_facts = true            # remember per project which check passed last time and which commands are
                                # missing, and tell the next session (~/.hame/facts/)
dedupe_results = true           # a tool result identical to one still in context is replaced by a pointer to it
condense_output = true          # drop passing-test and build-progress lines from command output; errors and
                                # summaries stay verbatim, the full output is saved to a file
defer_tools_over = 20           # with more MCP/plugin tools than this, they are not declared to the model;
                                # it finds them with tool_search and runs them with tool_call

[verify]
commands = ["cargo test -q"]    # must pass before a turn that changed files may end
checklist = false               # true: before finishing, the model goes through each point of the request
stop_on_red = true              # when the model means to stop with the last test/build still failing, or right
                                # after changing source without running anything: send it back (twice per turn at most)
review = false                  # true: before finishing, a fresh agent (seeing only the request and the files)
                                # looks for easily missed rules and reports violations to the model; once per turn,
                                # costs that agent's tokens

[limits]
tree_tokens = 2000000           # token budget for the agent and all its subagents; 0 = unlimited
subagent_depth = 1
max_steps = 100                 # most requests in one turn (hame run --max-steps overrides)

[mcp.docs]                      # an MCP server (stdio); runs in the sandbox by default
command = "docs-server"
args = ["--stdio"]

[mcp.tracker]                   # an MCP server over HTTP (streamable HTTP)
url = "https://mcp.example.com/mcp"
headers = { Authorization = "Bearer $TRACKER_TOKEN" }   # $NAME comes from the environment or a .env file

[lsp.rust]                      # language server for `diagnostics`, `references`, `rename_symbol`
command = "rust-analyzer"
extensions = ["rs"]

[[hook]]                        # a command run around each tool call; exit code 2 = objection
event = "pre_tool"
tool = "bash"
command = "./scripts/check-command.sh"

[compat]
claude_hooks = ["user"]         # run hooks written for Claude Code: "user" = ~/.claude/settings.json,
                                # "project" = the project's .claude/settings(.local).json; off by default
claude_plugins = ["/path/to/plugin"]   # plugins packaged for Claude Code: load their skills, hooks and MCP servers

[plugin.wc]                     # a WebAssembly plugin and what it may read
path = "plugins/wordcount.wasm"
read = ["docs"]

[computer]                      # browser and desktop tools; see "Browser and desktop control"
browser = false
desktop = false
headless = true
browser_channel = "chrome"      # chrome | msedge | chromium
# profiles_dir = "/path"        # default ~/.hame/browser-profiles
batch = true                    # batch action in browser and desktop tools
observation = "semantic"        # semantic | screenshot | pixels

[[computer.grant]]
channel = "browser"             # browser | desktop | visual
origin = "http://127.0.0.1:*"   # also: app = "com.apple.TextEdit", profile = "work"
capabilities = ["observe", "interact"]

[web]                           # hame web
idle_seconds = 600              # a session's agent is shut down (and its lock released) after this long without a turn

[remote]                        # devices reaching this computer through the relay (on top of all of the above)
allow_full_access = false       # true: a remote device may choose Full access or Accept edits, allow a tool for a
                                # whole session, and drive sessions that run unsandboxed without asking
roots = ["~/code"]              # folders a remote device may start sessions in, and (when set) the only sessions it
                                # can see; default: only folders that already have a session, and the folder hame web
                                # started in (and it sees every session)
```

When a tool call repeats exactly and returns the same result three times in a row, or the same call fails four times without ever succeeding, its result carries a note telling the model it is stuck (the note also counts toward escalation). From the fifth identical call in a row, the call is no longer run (polling commands repeated verbatim included). Three requests before a turn runs out of steps, the model is told so it can wrap up. Every model switch (escalation, and the return at the next turn) is logged.

MCP: stdio servers run in the sandbox; HTTP servers are called from the hame process and authenticated with static headers. The client speaks both the handshake versions (`initialize`, up to `2025-06-18`) and the stateless `2026-07-28` version: it tries the old handshake first, and when the server refuses, asks `server/discover` and then sends the version, identity and the `Mcp-Method`, `Mcp-Name` and `Mcp-Param-*` headers on each request. For servers that use OAuth, `hame mcp login <name>` finds the authorization server from the server itself, registers a client, opens the browser for you to sign in (authorization code with PKCE) and stores the token in `~/.hame/mcp-auth/` (readable only by you); tokens are refreshed when a session starts, not during one. This flow ran end to end with a real provider (Notion's MCP server, 2026-11-14): client registration, sign-in, token use, refresh of an expired token, and sign-out. For an authorization server that does not allow dynamic registration, register hame with the provider and set `oauth_client_id` (and `oauth_redirect_port`, the port of the redirect address `http://127.0.0.1:<port>/callback`) under `[mcp.<name>]`. Not supported yet: requests from the server back to the client (sampling, elicitation, roots) and running as background tasks; tools report these clearly. Both transports were tested against a server built with the official MCP SDK (`mcp` package 2.3.0), over stdio and over HTTP. Set `protocol = "2026-07-28"` under `[mcp.<name>]` to go straight to the stateless version without trying the old handshake.

Claude Code hooks: hame reads the same `settings.json` files and speaks the same protocol (JSON on stdin, exit code 2 to block, JSON on stdout) for the `PreToolUse`, `PostToolUse`, `UserPromptSubmit` and `Stop` events, with `command` hooks, `matcher` and `if`. Tool names and arguments are translated to Claude Code's (`Bash`, `Edit`, `file_path`...). Other events and hook types are reported once at startup and then ignored. Differences to know: hooks run one after another, not in parallel; `permissionDecision: "ask"` is treated as a block; `"allow"` does not bypass hame's permission rules; `tool_response` has only an `output` field; `transcript_path` points at hame's log, which is not in Claude Code's format. Hooks run outside the sandbox, so enable `"project"` only for repositories you trust.

Claude Code plugins: for each plugin installed with `hame plugin install` (in `~/.hame/plugins`) and each directory in `claude_plugins`, hame loads `skills/`, `commands/`, `agents/`, `hooks/hooks.json` (including `${CLAUDE_PLUGIN_ROOT}`) and the servers in `.mcp.json` (stdio and HTTP; tools are named `mcp__<plugin>_<server>__<tool>`). The installer scans the whole plugin as it does skills, and says when a plugin has hooks or MCP servers. The source can be a marketplace (a directory or git repository with `.claude-plugin/marketplace.json`): choose the plugin with `--plugin`; entries that point at another git repository are fetched at the commit the catalogue pins. `hame plugin update` fetches again from the remembered source and rescans before replacing; there are no automatic updates. Not supported: other plugin parts (output styles, themes, LSP) and plugins distributed through their own installers.

Commands and subagent kinds: a file `commands/<name>.md` in `.hame/`, `.claude/`, `~/.hame/` or a plugin is a prompt invoked as `/<name> args` (with `$ARGUMENTS`, `$1`...`$9`), usable from `hame run`, the terminal interface and ACP. A file `agents/<name>.md` (front matter `name`, `description`, `tools`) defines a kind of subagent the model picks through the `agent` argument of the `subagent` tool; it runs with the file's instructions and, if `tools` is given, only those tools.

A project can declare its own check commands in `.hame/config.toml` (the `[verify]` section only; a repository cannot loosen the sandbox). The project's `AGENTS.md` and `CLAUDE.md`, and `~/.hame/AGENTS.md`, are appended to the system prompt. Skills live in `.hame/skills/<name>/SKILL.md`, `.claude/skills/`, `.agents/skills/` (where `npx skills` installs them), `~/.hame/skills/` or `~/.agents/skills/`; a skill with `disable-model-invocation: true` is not offered to the model.

`hame skill install` reads every file of a skill before copying it: invisible characters, sentences telling the model to ignore instructions or hide things from the user, commands that download and pipe straight into a shell, code hidden in base64, paths to keys and secret files, symlinks. When it sees danger signs it does not install unless you add `--force`. This is pattern matching, not proof: a clean scan means no known trick was found, not that the skill is safe.

Example plugin and ABI description: `plugins/wordcount`.

## Sessions, artifacts and retention

Each session is a directory in `~/.hame/sessions/` holding its log (format version 3) and, for tools that act outside the sandbox (browser, desktop), an action journal (`actions.v1.jsonl`). Images and files that tools return (screenshots, downloads, images from MCP servers) are stored once, by SHA-256, in `~/.hame/artifacts/`, shared by all sessions, forks and imports; logs hold only typed references. The terminal interface shows each as a one-line description, the web interface and ACP clients show the image. Images reach the model as images when it accepts them (see [Troubleshooting](#troubleshooting)). The model is shown at least the 3 most recent images from tool results; older ones become a note naming the artifact, dropped three at a time so the cached prompt prefix stays the same between steps. Resuming a session warns about artifacts that are missing or damaged; the session still works and the model gets a note in their place. Nothing is deleted automatically: `hame sessions gc` removes artifacts no session log refers to that are older than 24 hours (`--older-than-hours N`, `--dry-run` to only report), and temporary files of interrupted writes. A crash between storing an image and logging it leaves such an unreferenced file. Deleting a session directory makes its artifacts collectable. There is no export command; to move a session, copy its directory together with `~/.hame/artifacts/`. After a crash, actions that were dispatched but never confirmed are recorded as `outcome_unknown` and the model is told to check the current state before retrying; hame never repeats them itself. A fork carries every settled action receipt of its source, since forking undoes nothing outside the workspace.

A log written by this version (format 3) is refused by older builds ("session format version 3 is newer than this build supports"); older logs open unchanged. An older log stays at its version until a record only format 3 has (a `mode/changed` record, written when the web interface's permission mode changes) is added to it: its header is then rewritten to version 3 first (atomically, under the session's lock), so an older hame reports it as newer rather than corrupt.

## Traces and evaluation

`hame run --trace FILE` and `hame task --trace FILE` write a trace (`trace.v1.jsonl`): one span per turn, model attempt (retries and failed attempts included), tool call (including steps inside `script`, `batch` and workflows) and verification, with timings and status, and one usage record per attempt with the provider's token counts, normalized and raw. `--trace session` puts it beside the session log; `HAME_TRACE` sets it for `hame run`. A trace holds names, ids, sizes and timings, never raw tool input or provider error bodies, and never enters the prompt. Nothing is sent anywhere: hame has no telemetry that leaves your machine.

`hame eval` reads those traces to report tokens, requests, request bytes and cost per run. Costs come from a dated tariff table (Anthropic models, 2026-11-15); a model not in the table has an unknown cost, never zero. To compare configurations on the same tasks:

```sh
hame eval evals/pilot --arm base --arm lean=lean.toml --runs 3 --json out/pilot.json
hame eval evals/pilot --arm base --arm nodedupe --set nodedupe:context.dedupe_results=false
```

Arms run interleaved; each is compared with the first, task by task, with a bootstrap confidence interval (`--bootstrap 2000`, `--seed 1`). Each run is recorded with its outcome (`pass`, `fail`, `false_success` when the agent claimed success and the hidden check failed, `timeout`, `error`, `unsupported`), the agent's time, the checker's time and their sum. Task manifests are described in [evals/pilot/README.md](evals/pilot/README.md); measured results are in [docs/release/EVIDENCE.md](docs/release/EVIDENCE.md).

## Troubleshooting

Start with `hame doctor`: every line that is not `ok` says what to do.

- **`browser`: "Playwright for Python is not importable by python3".** The browser helper runs `python3 -I` (isolated mode), which ignores `PYTHONPATH` and the per-user site directory, so `pip install --user playwright` and packages found only through `PYTHONPATH` are not seen. Install Playwright into the interpreter that is `python3` on hame's `PATH`. With a Homebrew or system Python that refuses `pip install` (PEP 668), use a virtual environment and put it first on `PATH` when you start hame:

  ```sh
  python3 -m venv ~/.hame/venv
  ~/.hame/venv/bin/pip install playwright
  export PATH="$HOME/.hame/venv/bin:$PATH"
  python3 -I -c "import playwright.async_api" && echo ok   # what hame checks
  ```
- **`browser`: "Google Chrome is not installed".** Install Chrome, or set `browser_channel = "chromium"` under `[computer]` after `python3 -m playwright install chromium`.
- **`desktop`: `permission_denied`, or doctor says Accessibility is not granted.** macOS grants Accessibility and Screen Recording to the app that started hame (Terminal, iTerm, your editor), not to hame. Add that app under System Settings > Privacy & Security > Accessibility (and Screen & System Audio Recording for screenshots), then quit and reopen it. A screenshot that comes back as one flat colour is refused as `blank_capture`; that is usually a missing or stale Screen Recording grant.
- **`desktop`: the helper does not build.** Install the command line tools: `xcode-select --install`. The helper is cached in `~/.hame/helpers/` and rebuilt when hame's copy changes.
- **`not_authorized: ... outside the granted scope`.** The action needs a capability no grant covers for that app, origin or profile. Add a grant to `[computer]` or to the task file; text on screen cannot grant it.
- **The model does not see screenshots ("not sent, ...").** Images go to a model only when hame knows it accepts them: Claude models, OpenAI's gpt-4o, gpt-4.1, gpt-4.5, gpt-4-turbo, gpt-5, o1, o3 and o4 families, and, behind a compatible server, names starting with `gemini` or containing `-vl` or `vision`. Others, DeepSeek included, are treated as text-only and each image becomes a short note (the image is still stored). If your model takes images, set `vision = true` (and `pdf = true` for PDFs) under `[model]`; if it does not, choose a model that does or let the agent work from page structure and accessibility data.
- **`Workflow ... is not trusted in this exact version`.** Review it with `hame workflow show NAME` and run `hame workflow trust NAME`. An edited file needs trusting again.
- **"session format version N is newer than this build supports".** The log was written by a newer hame; use that version.

## Limitations

- **Desktop control is macOS only** (14 or later) and was tested on macOS 26.6.2 with TextEdit, Numbers 14.4 and Pages 14.4. Excel exposes no cell grid to accessibility; Word and Keynote were not tried. Not implemented: scrolling, drag, hover, choosing pop-up menu items by value, context menus. A desktop `file_write` grant is limited to folders by the names the save panel shows, so another folder with the same name and parent name would pass. Commands in the shell sandbox cannot send Apple Events (`osascript`), use the accessibility interface, post or tap input events, capture the screen or use the clipboard, so other apps are driven only through the desktop tool; a program run inside the sandbox that needs a window server (a GUI app, a browser) does not work there. `key` assumes a US (ANSI) layout; secure text fields generally ignore synthesized input; windows on another Space or minimized cannot be captured.
- **The browser tool** drives Chrome-family browsers through Playwright and was verified on macOS arm64 only. Iframes and closed shadow roots are not expanded; commit detection is heuristic (above); a page's own network requests are not filtered.
- **Outcome checks** see what the predicates name: a task file without a check for some side effect does not verify it. `desktop_state` reads values cut at 300 characters; check long documents in the saved file. False-success detection in `hame eval` is phrase-based.
- **Budgets**: prices are known only for Anthropic models, so `max_cost_usd` stops a task on any other provider at its first request.
- **Live provider tests** covered Anthropic (images in tool results and messages, missing-image notes, prompt cache) with claude-haiku-5-5. OpenAI image and PDF paths, and Anthropic PDFs, are tested against recorded fixtures only. See [docs/release/SUPPORT-MATRIX.md](docs/release/SUPPORT-MATRIX.md).
- No installers or prebuilt binaries yet; build from source. hame has not had a security audit.

## Code layout

| Crate | Job |
|---|---|
| `hame-kernel` | Service registry, event bus, middleware chain |
| `hame-session` | Event schema, JSONL store, crash recovery, fork, projection; artifact store; task, grant, observation and receipt contracts; action journal |
| `hame-llm` | Adapters for OpenAI Responses, Anthropic Messages and chat completions (DeepSeek, compatible servers); native images and PDFs; model profiles |
| `hame-tools` | Built-in tools and the `Tool` trait |
| `hame-agent` | Turn/step loop, hooks, checkpoints |
| `hame-sandbox` | Policy, Seatbelt (macOS), Landlock (Linux), integrity levels (Windows), worker, egress proxy |
| `hame-policy` | Configuration, permission rules, approval, secret masking |
| `hame-checkpoint` | Workspace checkpoints in a separate git store |
| `hame-context` | Output spilling, context compaction, project instructions |
| `hame-verify` | Completion gate before a turn ends; loop detection; model escalation; outcome predicates, task files, budgets, workflows |
| `hame-browser` | The `browser` tool over a persistent Playwright helper |
| `hame-desktop` | The `desktop` tool over a Swift accessibility helper (macOS) |
| `hame-trace` | Run traces, usage accounting, dated tariff |
| `hame-ext` | MCP, LSP (diagnostics, references, symbol rename), skills, hooks, WebAssembly plugins |
| `hame-multi` | Subagents, worktrees, task board, shared budget |
| `hame-app` | Assembles a complete agent from configuration; session import |
| `hame-acp` | Agent Client Protocol server; the terminal interface is also a client of it |
| `hame-tui`, `hame-web`, `hame-cli` | The interfaces |
| `hame-eval` | Scripted model that replays recordings; task manifests, run records, arms and bootstrap comparison |

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Plugin tests need `rustup target add wasm32-unknown-unknown`; some tests need `python3` and `git`. Set `HAME_SCRIPT=<file.jsonl>` to make `hame` replay recorded responses instead of calling a provider. Browser tests skip themselves without Playwright and Chrome; the desktop GUI tests are opt-in (`--ignored`). Release tooling (`scripts/docker-clean-install.sh`, `scripts/license-report.py`, `scripts/release-check.sh`) and the release documents are in [docs/release/](docs/release/). See [CONTRIBUTING.md](CONTRIBUTING.md) and [CHANGELOG.md](CHANGELOG.md).

## License

MIT; see [LICENSE](LICENSE).
