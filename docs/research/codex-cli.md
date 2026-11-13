# OpenAI Codex CLI: market research (as of 2026-11-14)

Web search/fetch, `gh` and the local binary (`codex-cli 0.167.2`) all worked. Latest release is `rust-v0.169.0` (2026-11-13); the repo has 135k stars, Apache-2.0 (`gh repo view openai/codex`). Official docs now redirect from `developers.openai.com/codex/*` to `learn.chatgpt.com/docs/*`.

Source shorthand: **[docs:x]** = `https://learn.chatgpt.com/docs/x`, **[repo:x]** = path in `openai/codex`, **[local:x]** = command run here, **[rel X]** = GitHub release notes for `rust-v0.X.0`.

## 1. Architecture
- **Workspace size:** `codex-rs` has about 130 crates [repo:codex-rs]. The main ones:
  - Agent logic: `core` (session, turn, `compact*.rs`, `guardian/`, `unified_exec/`, `tools/`), `protocol`, `tools`.
  - Front ends: `tui`, `exec`, `cli`.
  - Server layer: `app-server`, `app-server-protocol`, `app-server-daemon`, `exec-server`.
  - Persistence: `rollout`, `thread-store`, `state` (SQLite migrations).
  - Sandboxing: `sandboxing`, `linux-sandbox`, `windows-sandbox-rs`, `mxc-sandbox`, `network-proxy`, `execpolicy`.
  - Extensions: `hooks`, `skills`, `plugin`, `memories/{read,write}`, `code-mode*`, `ext/*`.
- **App-server protocol:** JSON-RPC 2.0 with the `jsonrpc` field omitted. Transports are JSONL over `stdio://`, `ws://`, or WebSocket over `unix://`. Handshake is `initialize` then `initialized`. Primitives are Thread, Turn, Item [docs:app-server; local:`codex app-server --help`].
  - Methods include `thread/start|resume|fork|revert|compact/start`, `turn/start|steer|interrupt`, `review/start`.
  - Server-to-client requests include `item/commandExecution/requestApproval` and `item/permissions/requestApproval`.
  - Schemas come from `codex app-server generate-ts` / `generate-json-schema`.
- **Clients:** the TUI is itself an app-server client (feature `tui_app_server` is "removed/true"; `--remote ws://…` attaches to a remote server) [local:`codex features list`, `codex --help`]. The VS Code extension is built on app-server [docs:app-server]. Desktop app internals are closed (unverified).
- **Daemon:** since 0.157 interactive sessions auto-start a shared background daemon (`daemon_auto_start`); `--no-daemon` bypasses it [rel 157; local:`codex app-server daemon --help`].
- **Remote execution:** `codex exec-server` is a separate JSON-RPC process/filesystem executor, with Noise-encrypted relays since 0.141 [repo:codex-rs/exec-server/README.md; rel 141].
- **Cloud:** `codex cloud exec|status|list|apply|diff` [local:`codex cloud --help`].
- **Storage boundary:** the `ThreadStore` trait, with `LocalThreadStore` writing JSONL rollouts plus SQLite metadata [repo:codex-rs/thread-store/README.md].

## 2. Sandboxing
- **Modes:** `read-only`, `workspace-write`, `danger-full-access`. These are being replaced by named permission profiles: `default_permissions = ":read-only" | ":workspace" | ":danger-full-access"` or a custom `[permissions.<name>]` [docs:config-file/config-reference].
  - Profiles support `extends`, `filesystem.<path|glob> = read|write|deny`, `:workspace_roots`, and `network.domains.<pattern> = allow|deny`.
- **Protected paths:** `.git` (including the resolved `gitdir:`), `.codex`, `.agents` and `.aws` stay read-only inside writable roots [docs:agent-approvals-security].
- **macOS:** `/usr/bin/sandbox-exec` with generated Seatbelt policies (`seatbelt_base_policy.sbpl`, `seatbelt_network_policy.sbpl`) [repo:codex-rs/sandboxing/src, codex-rs/core/README.md].
- **Linux:** bubblewrap plus in-process seccomp [repo:codex-rs/linux-sandbox/README.md].
  - Uses system `bwrap`, else a bundled one. Mounts `--ro-bind / /`, then `--bind` for writable roots, then re-applies `--ro-bind` on protected subpaths.
  - Sets `PR_SET_NO_NEW_PRIVS`; seccomp blocks network syscalls and `io_uring`.
  - Deny globs are expanded with `rg` before launch.
  - Landlock is legacy and rejected for restricted filesystem policies. WSL1 is unsupported.
  - Ubuntu 24.04 needs an AppArmor profile for user namespaces [docs:sandboxing].
- **Windows:** three backends [docs:windows/windows-sandbox].
  - `mxc`: Microsoft MXC `BaseContainerRunner`. No admin, accounts or firewall rules, but needs Windows 11 24H2 build 26100.9278 or later. Detached child processes die when the foreground command exits; hard-link aliasing is an accepted gap [repo:codex-rs/mxc-sandbox/README.md].
  - Legacy `elevated`: a UAC setup creates local users `CodexSandboxOffline` and `CodexSandboxOnline`, hidden from Winlogon. It adds firewall rules such as `codex_sandbox_offline_block_outbound` and loopback blocks that except the proxy port, plus WFP filters and ACLs [repo:codex-rs/windows-sandbox-rs/src/setup.rs, setup_provisioning/firewall.rs, wfp.rs, hide_users.rs]. Removed with `codex sandbox uninstall` [local].
  - Legacy `unelevated`: restricted token plus ACLs, weaker network isolation, no deny-read.
  - Selected with `[windows] sandbox = "mxc"|"elevated"|"unelevated"` and `features.prefer_mxc`.
- **Network:** off by default in workspace-write. An optional managed proxy (`features.network_proxy`, HTTP on 3128 and SOCKS5) applies domain allow/deny, with deny winning, and blocks non-public DNS results [docs:agent-approvals-security].

## 3. Approvals
- **Policies:** `approval_policy = on-request | never | { granular = { sandbox_approval, rules, mcp_elicitations, request_permissions, skill_approval } }`. `untrusted` is retired and `on-failure` deprecated [docs:config-file/config-reference].
- **Escalation:** the model sets `sandbox_permissions: "require_escalated"` with a `justification` and an optional reusable `prefix_rule` on `exec_command`. A `request_permissions` tool exists but is still under development [repo:codex-rs/core/src/tools/handlers/shell_spec.rs; local:`codex features list`].
- **Rules:** Starlark `prefix_rule(pattern, decision = allow|prompt|forbidden, justification, match, not_match)` in `~/.codex/rules/*.rules` and trusted `<repo>/.codex/rules/` [docs:agent-configuration/rules].
  - Strictest match wins.
  - `bash -lc` scripts are split only when they are plain chains of `&&`, `||`, `;`, `|`.
  - Test with `codex execpolicy check`.
- **Auto-review ("Guardian"):** `approvals_reviewer = "auto_review"` or `--approve-for-me` sends approval requests to a separate reviewer agent [docs:sandboxing/auto-review].
  - The bundled catalog includes a hidden `codex-auto-review` model.
  - It sees a compact transcript without reasoning and can run read-only checks.
  - Failures fail closed.
  - Circuit breaker: 3 consecutive denials, or 10 in the last 50 reviews, interrupts the turn.
  - Policy is overridable via `[auto_review].policy`.
- **Managed policy:** `requirements.toml` (`allowed_approval_policies`, `allowed_sandbox_modes`, `prompt-user` human checkpoints) [repo:codex-rs/app-server/README.md].

## 4. Tools
- **Shell:** `exec_command` (`cmd`, `workdir`, `tty`, `yield_time_ms`, `max_output_tokens`, `login`) plus `write_stdin(session_id, chars)` for PTY-backed background processes [repo:…/handlers/shell_spec.rs]. `features.unified_exec` is on and cannot be opted out; `background_terminal_max_timeout` defaults to 300 s [docs:config-file/config-reference].
- **`apply_patch`:** a freeform custom tool constrained by a Lark grammar (`assets/tools/apply_patch.lark`, `start: begin_patch hunk+ end_patch`), not JSON [repo:…/handlers/apply_patch_spec.rs].
- **Planning and interaction:** `update_plan` (steps with `pending|in_progress|completed`), `request_user_input`, `view_image`, `sleep`, `tool_search` for deferred MCP tools (default since 0.143), `get_context_remaining`, `new_context_window` [repo:codex-rs/core/src/tools/handlers].
- **Hosted tools:** Responses `web_search` with `web_search = disabled|cached|indexed|live`, default `cached`; image generation.
- **Code mode** (`features.code_mode`, under development): an `exec` freeform tool runs JavaScript in a fresh V8 isolate, with a `wait` tool for long-running cells [repo:codex-rs/code-mode-protocol/src/description.rs].
  - All tools are exposed as `await tools.<name>()`.
  - No Node, filesystem or network.
  - Helpers: `text()`, `store/load`, `yield_control()`, `tools.tool_search`.
- **Multi-agent:** `spawn_agent`, `send_input`/`send_message`, `followup_task`, `wait_agent`, `interrupt_agent`, `list_agents`, `close_agent` [repo:…/handlers/multi_agents_spec.rs].

## 5. Context management
- **Auto-compaction:** triggers at min(config, 90% of context window) [repo:codex-rs/protocol/src/openai_models.rs].
  - Keys: `model_auto_compact_token_limit`, `compact_prompt`.
  - Local summarisation uses `prompts/templates/compact/prompt.md`; server-side "remote compaction v2" also exists (`compact_remote_v2*.rs`); background `async_compaction` is in development.
  - `PreCompact` and `PostCompact` hooks fire around it.
- **Tool output:** truncated at 10,000 tokens per the model catalog `truncation_policy` [repo:codex-rs/models-manager/models.json]; adjustable with `tool_output_token_limit` and per-MCP-tool `output_token_limit`.
- **Caching and transport:** `prompt_cache_key` per thread; Responses WebSocket (`responses_websockets=2026-02-06`) reuses the connection and `previous_response_id` for incremental requests, with HTTP fallback [repo:codex-rs/core/src/client.rs].
- **AGENTS.md:** global `~/.codex/AGENTS(.override).md`, then one file per directory from repo root to cwd, concatenated, capped at 32 KiB (`project_doc_max_bytes`). Built once per session; nested files are not loaded dynamically [docs:agent-configuration/agents-md].
- **Memories** (off by default): a two-phase background pipeline does per-rollout extraction, then a consolidation agent writes `$CODEX_HOME/memories_v2/` [repo:codex-rs/memories/README.md; docs:customization/memories].
- **Sessions:** stored at `~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl.zst`, with SQLite alongside (`state_5.sqlite`, `thread_history_1.sqlite`) [local:`ls ~/.codex`].
  - Commands: `codex resume|fork|archive|delete|migrate-rollouts`.
  - `thread/rollback` was removed in favour of `thread/revert`. `/undo` was removed (feature `undo` = removed).

## 6. Extensibility
- **MCP client:** stdio and HTTP with OAuth, `enabled_tools`, per-tool `approval_mode`, elicitation [docs:config-file/config-reference]. The MCP server mode (`codex mcp-server`) has been removed; app-server replaces it [docs:mcp-server].
- **Hooks:** 14 events (`PreToolUse`, `PermissionRequest`, `PostToolUse`, `UserPromptSubmit`, `Stop`, `SubagentStart/Stop`, `SessionStart/End`, `Pre/PostCompact`, `Pre/PostModelRequest`, `Interrupt`) [docs:hooks].
  - Configured in `hooks.json` or `[hooks]`.
  - Handler types are `command` and `mcp_tool`, with `async` available.
  - Trust is hash-based.
  - The schema is Claude-Code-compatible, including a `CLAUDE_PLUGIN_ROOT` alias.
- **Skills:** `SKILL.md` in `.agents/skills`, `~/.agents/skills`, `/etc/codex/skills`. The initial listing is budgeted at 2% of context [docs:build-skills].
- **Plugins:** `.codex-plugin/plugin.json`, marketplaces, `codex plugin add|list|marketplace` [local]. Custom prompts are deprecated [docs:custom-prompts].
- **Subagents:** roles `default`, `worker`, `explorer`; custom TOML in `.codex/agents/`; they inherit the parent sandbox [docs:agent-configuration/subagents].
- **Profiles:** `$CODEX_HOME/<name>.config.toml` via `-p` [local].
- **SDKs:** TypeScript `@openai/codex-sdk` spawns the CLI and exchanges JSONL; Python is `openai-codex` [repo:sdk/*/README.md].
- **Non-interactive:** `codex exec --json` emits `thread.started`, `turn.*`, `item.*`; also `--output-schema` and `--ephemeral` [repo:codex-rs/exec/src/exec_events.rs].
- **Other:** `openai/codex-action` (inputs include `safety-strategy`, `permission-profile`); `--worktree` with managed worktrees in `$CODEX_HOME/worktrees` and `.worktreeinclude` [docs:environments/git-worktrees].

## 7. Models and providers
- **Wire API:** Responses only (`wire_api = "responses"`) [docs:config-file/config-reference].
- **Providers:** built-ins are `openai`, `ollama`, `lmstudio` (`--oss`, `--local-provider`), `amazon-bedrock` and `gcp` (Vertex, 0.165). Custom `model_providers.<id>` take `base_url`, `env_key`, `auth.command`.
- **Bundled catalog:** `gpt-6.1-sol` (default since 0.161), `gpt-6.1-terra` (500k context), `gpt-6-astra`, `gpt-6-sol/luna`, `gpt-5.6-*`, `gpt-5.5` [repo:codex-rs/models-manager/models.json].
- **Reasoning effort:** `low|medium|high|xhigh|max|ultra`, changeable mid-conversation (0.165). `service_tier = fast`. `plan_mode_reasoning_effort`.
- **Per-model harness behaviour** is driven by catalog fields: `shell_type`, `apply_patch_tool_type`, `tool_mode`, `multi_agent_version`.

## 8. Shipped in the last six months (GitHub release notes)
| Version | Date | Notable |
|---|---|---|
| 0.131 | 05-18 | `codex doctor`, Python SDK renamed, `remote-control` daemon |
| 0.133 | 05-21 | Goals on by default; permission-profile inheritance |
| 0.141 | 06-18 | Noise-encrypted remote executors |
| 0.142 | 06-22 | Rollout token budgets, `indexed` web search |
| 0.143 | 07-08 | Tool search default for MCP; system proxy support |
| 0.145 | 07-21 | Paginated thread history; `/import` from Cursor and Claude Code; multi-agent v2 |
| 0.147 | 08-07 | `--approve-for-me`; MCP 2026-07-28 opt-in |
| 0.148 | 08-18 | `/export`, `codex exec fork`, async and MCP hooks |
| 0.149 | 08-20 | `codex agents` dashboard, `codex queue` |
| 0.154 | 09-09 | GPT-6-Astra, `--worktree`, Windows shared daemon |
| 0.156–0.157 | 09-22/25 | Fullscreen TUI, voice, daemon auto-start, GPT-6 Sol/Luna |
| 0.162 | 10-08 | Worktree tools; MXC check (per docs) |
| 0.163 | 10-15 | `/config`, mid-turn steering, parallel read-only tools |
| 0.165 | 10-27 | Vertex provider, memory v2 default |
| 0.168–0.169 | 11-13 | Side-chat panes, provider picker |

## 9. Weaknesses and complaints
- **Scale of backlog:** 27,374 open issues, of which 9,521 are labelled `windows-os`, 2,556 `sandbox` and 1,955 `rate-limits` (`gh api search/issues`).
- **No undo or checkpoint restore:** #9203 "make /undo back" (498 thumbs-up) and #11626 `/rewind` (231).
- **Windows sandbox fragility:** #51601 (setup sharing violation, 125 comments), #29072, #10601, #48043, and a data-loss report, #46022.
- **Compaction quality and control:** #4106 (129 thumbs-up), #5957, #29356, #33493.
- **Unwanted daemon:** #48195 (192 thumbs-up) objects to a persistent self-updating daemon that copies about 374 MB.
- **Resource leaks:** #30408, MCP processes per thread never cleaned up.
- **Opaque multi-agent:** #28058, encrypted v2 messages remove the audit trail (139 thumbs-up).
- **Quota opacity:** #41220 meta-tracker; #57084 on 429s.
- **Missing features:** LSP (#8745), dynamic nested AGENTS.md (#12115), IDE diff approval (#2998).
- **Third-party providers:** #26234 (namespaced tools) and #31870 (Azure) show breakage from Responses-only assumptions.

## Techniques worth copying
- One JSON-RPC app-server with generated TypeScript and JSON Schema, used by every front end, with Thread/Turn/Item primitives.
- Grammar-constrained freeform tools for patches and code.
- Composable permission profiles (path and glob read/write/deny, domain rules) compiled to each OS backend.
- Protected metadata paths inside writable roots.
- A model-proposed `prefix_rule` on escalation, persisted to Starlark rules that carry self-testing `match`/`not_match` examples.
- A reviewer agent for approvals with fail-closed behaviour and a denial circuit breaker.
- A data-driven model catalog that selects tool variants and truncation per model.
- Deferred MCP tools behind `tool_search`.
- PTY sessions with `yield_time_ms` and `write_stdin`.
- Hash-pinned hook trust and a Claude-compatible hook schema; `/import` from competitors.
- JSONL plus SQLite storage behind a `ThreadStore` trait.

## Weaknesses a competitor could exploit
- No file-level undo or checkpoints.
- Windows needs admin setup, hidden local accounts and firewall rules, or a very recent Windows 11 build.
- Responses-only wire API, so Chat Completions and Anthropic-style providers are second-class.
- Compaction is lossy and offers little user control.
- Heavy default footprint: daemon, 130 crates, a huge flag surface (about 170 feature flags) and roughly two releases a week with regressions.
- Static AGENTS.md with a 32 KiB cap.
- Subagents cannot have a narrower sandbox than the parent.
- Hooks are explicitly "not a complete enforcement boundary" [docs:hooks].
- No LSP integration.
- Docs and product are tied to ChatGPT accounts and quotas.
