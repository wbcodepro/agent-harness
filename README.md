# hame

English · [Tiếng Việt](README.vi.md)

`hame` is a coding-agent harness written in Rust: one binary that runs a language model as a coding agent in your terminal, in your editor (over ACP) or in a browser.

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

hame needs Rust 1.89 or later, and `git`.

```sh
cargo install --git https://github.com/wbcodepro/agent-harness hame-cli
# or, from a clone:
cargo build --release -p hame-cli      # builds target/release/hame
```

Then run `hame setup`. It asks which provider to use and for an API key; the key is not shown on screen and is saved in `~/.hame/.env`, readable only by you. It then checks the key, the sandbox, `bash` and `git`, and can send one test request that costs a few tokens. `hame doctor` runs the same checks at any time (`--ping` sends the test request).

You can also put the key in the environment or in `~/.hame/.env` (or `.env` in the working directory) yourself:

```sh
DEEPSEEK_API_KEY=...     # or ANTHROPIC_API_KEY / CLAUDE_API_KEY, OPENAI_API_KEY
```

## Use

```sh
hame setup                            # choose a provider, save its key, check everything
hame doctor [--ping]                  # check configuration, key, sandbox and required programs
hame                                  # terminal interface
hame run "fix the bug that makes test_stats.py fail"   # one turn, non-interactive
hame run --session <id> "carry on"    # continue a session
hame run --plan "..."                 # investigate and propose a plan; change nothing
hame run --output json "..."          # one JSON object when done (stream-json: also one event per line)
hame web                              # web interface on 127.0.0.1
hame acp                              # Agent Client Protocol server for editors
hame sessions | show <id> | fork <id> | rewind <id> | recover <id>
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

## Built-in tools

`read`, `write`, `edit`, `bash`, `job_output`, `glob`, `grep`, `repo_map`, `subagent`, `task_create` / `task_update` / `task_list`; `skill` when there are skills; `diagnostics`, `references` and `rename_symbol` when a language server is configured. With `edit_format = "apply-patch"`, `apply_patch` replaces `edit` and `write`.

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

`--sandbox read-only` forbids writes; `--sandbox danger-full-access` removes confinement, and then every tool that is not read-only needs your approval.

Known limits:

- The temp directories are writable by default (`[sandbox] writable_tmp = false` turns that off). A project inside `/tmp` can write to its sibling directories.
- On Linux, Landlock cannot hide files by name: a `.env` inside the workspace stays readable (its values are still masked in the log); the parent directory of a hidden path cannot be listed; the network is limited by TCP port, and UDP is not blocked. Needs Linux 6.7 or later.
- On Windows only **writes** are confined: the process running tools is lowered to Low integrity, so it can write only in the project directory and a private temp directory (`AppData\LocalLow\hame-tmp`). Tools can still read every file you can, except hame's own keys, settings and sessions (labelled so that Low-integrity processes cannot read them), and they reach the network, so this must be accepted explicitly with `allow_partial = true` under `[sandbox]` (`hame setup` asks, or takes `--allow-partial` when run without a terminal); otherwise the confined modes refuse to start. The project directory is labelled Low permanently (other Low programs of the same user can write to it too). `AppData\LocalLow` is writable as well, since Windows labels it Low. Files or directories **moved** (not copied) into the project after the first run keep their old label and cannot be changed by tools; relabel with `icacls <project dir> /setintegritylevel "(OI)(CI)L" /T`. The `bash` tool uses Git for Windows' bash, found through `git` on PATH. That bash keeps state shared per installation: while another Git Bash is open outside the sandbox, bash inside it cannot start (the tool says so); close it, or set `HAME_BASH` to the `bash.exe` of a second Git installation used only by hame. The network proxy cannot be enforced on Windows; it starts only with `[secrets] substitute`, and only programs that honour the proxy variables use it (Python, Node, git; the curl shipped with Git for Windows rejects the proxy's certificate). A program that bypasses the proxy can reach any host but only holds the stand-in. Tested on GitHub Actions and on a real Windows 11 machine; results and what is untested are in `docs/WINDOWS-RESULTS.md`.
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
```

When a tool call repeats exactly and returns the same result three times in a row, or the same call fails four times without ever succeeding, its result carries a note telling the model it is stuck (the note also counts toward escalation). From the fifth identical call in a row, the call is no longer run (polling commands repeated verbatim included). Three requests before a turn runs out of steps, the model is told so it can wrap up. Every model switch (escalation, and the return at the next turn) is logged.

MCP: stdio servers run in the sandbox; HTTP servers are called from the hame process and authenticated with static headers. The client speaks both the handshake versions (`initialize`, up to `2025-06-18`) and the stateless `2026-07-28` version: it tries the old handshake first, and when the server refuses, asks `server/discover` and then sends the version, identity and the `Mcp-Method`, `Mcp-Name` and `Mcp-Param-*` headers on each request. For servers that use OAuth, `hame mcp login <name>` finds the authorization server from the server itself, registers a client, opens the browser for you to sign in (authorization code with PKCE) and stores the token in `~/.hame/mcp-auth/` (readable only by you); tokens are refreshed when a session starts, not during one. This flow ran end to end with a real provider (Notion's MCP server, 2026-11-14): client registration, sign-in, token use, refresh of an expired token, and sign-out. For an authorization server that does not allow dynamic registration, register hame with the provider and set `oauth_client_id` (and `oauth_redirect_port`, the port of the redirect address `http://127.0.0.1:<port>/callback`) under `[mcp.<name>]`. Not supported yet: requests from the server back to the client (sampling, elicitation, roots) and running as background tasks; tools report these clearly. Both transports were tested against a server built with the official MCP SDK (`mcp` package 2.3.0), over stdio and over HTTP. Set `protocol = "2026-07-28"` under `[mcp.<name>]` to go straight to the stateless version without trying the old handshake.

Claude Code hooks: hame reads the same `settings.json` files and speaks the same protocol (JSON on stdin, exit code 2 to block, JSON on stdout) for the `PreToolUse`, `PostToolUse`, `UserPromptSubmit` and `Stop` events, with `command` hooks, `matcher` and `if`. Tool names and arguments are translated to Claude Code's (`Bash`, `Edit`, `file_path`...). Other events and hook types are reported once at startup and then ignored. Differences to know: hooks run one after another, not in parallel; `permissionDecision: "ask"` is treated as a block; `"allow"` does not bypass hame's permission rules; `tool_response` has only an `output` field; `transcript_path` points at hame's log, which is not in Claude Code's format. Hooks run outside the sandbox, so enable `"project"` only for repositories you trust.

Claude Code plugins: for each plugin installed with `hame plugin install` (in `~/.hame/plugins`) and each directory in `claude_plugins`, hame loads `skills/`, `commands/`, `agents/`, `hooks/hooks.json` (including `${CLAUDE_PLUGIN_ROOT}`) and the servers in `.mcp.json` (stdio and HTTP; tools are named `mcp__<plugin>_<server>__<tool>`). The installer scans the whole plugin as it does skills, and says when a plugin has hooks or MCP servers. The source can be a marketplace (a directory or git repository with `.claude-plugin/marketplace.json`): choose the plugin with `--plugin`; entries that point at another git repository are fetched at the commit the catalogue pins. `hame plugin update` fetches again from the remembered source and rescans before replacing; there are no automatic updates. Not supported: other plugin parts (output styles, themes, LSP) and plugins distributed through their own installers.

Commands and subagent kinds: a file `commands/<name>.md` in `.hame/`, `.claude/`, `~/.hame/` or a plugin is a prompt invoked as `/<name> args` (with `$ARGUMENTS`, `$1`...`$9`), usable from `hame run`, the terminal interface and ACP. A file `agents/<name>.md` (front matter `name`, `description`, `tools`) defines a kind of subagent the model picks through the `agent` argument of the `subagent` tool; it runs with the file's instructions and, if `tools` is given, only those tools.

A project can declare its own check commands in `.hame/config.toml` (the `[verify]` section only; a repository cannot loosen the sandbox). The project's `AGENTS.md` and `CLAUDE.md`, and `~/.hame/AGENTS.md`, are appended to the system prompt. Skills live in `.hame/skills/<name>/SKILL.md`, `.claude/skills/` or `~/.hame/skills/`.

`hame skill install` reads every file of a skill before copying it: invisible characters, sentences telling the model to ignore instructions or hide things from the user, commands that download and pipe straight into a shell, code hidden in base64, paths to keys and secret files, symlinks. When it sees danger signs it does not install unless you add `--force`. This is pattern matching, not proof: a clean scan means no known trick was found, not that the skill is safe.

Example plugin and ABI description: `plugins/wordcount`.

## Code layout

| Crate | Job |
|---|---|
| `hame-kernel` | Service registry, event bus, middleware chain |
| `hame-session` | Event schema, JSONL store, crash recovery, fork, projection |
| `hame-llm` | Adapters for OpenAI Responses, Anthropic Messages and chat completions (DeepSeek, compatible servers) |
| `hame-tools` | Built-in tools and the `Tool` trait |
| `hame-agent` | Turn/step loop, hooks, checkpoints |
| `hame-sandbox` | Policy, Seatbelt (macOS), Landlock (Linux), integrity levels (Windows), worker, egress proxy |
| `hame-policy` | Configuration, permission rules, approval, secret masking |
| `hame-checkpoint` | Workspace checkpoints in a separate git store |
| `hame-context` | Output spilling, context compaction, project instructions |
| `hame-verify` | Completion gate before a turn ends; loop detection; model escalation |
| `hame-ext` | MCP, LSP (diagnostics, references, symbol rename), skills, hooks, WebAssembly plugins |
| `hame-multi` | Subagents, worktrees, task board, shared budget |
| `hame-app` | Assembles a complete agent from configuration; session import |
| `hame-acp` | Agent Client Protocol server; the terminal interface is also a client of it |
| `hame-tui`, `hame-web`, `hame-cli` | The interfaces |
| `hame-eval` | Scripted model that replays recordings, for tests and evals |

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Plugin tests need `rustup target add wasm32-unknown-unknown`; some tests need `python3` and `git`. Set `HAME_SCRIPT=<file.jsonl>` to make `hame` replay recorded responses instead of calling a provider. See [CONTRIBUTING.md](CONTRIBUTING.md) and [CHANGELOG.md](CHANGELOG.md).

## License

MIT; see [LICENSE](LICENSE).
