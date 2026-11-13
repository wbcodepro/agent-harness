# Claude Code market research (as of 2026-11-14, v2.1.323)

Web search/fetch, `gh` and the local binary all worked. Source shorthand: `docs/X` = `https://code.claude.com/docs/en/X.md` (fetched today); `CL x` = `CHANGELOG.md` in `anthropics/claude-code` at version x; `help` = local `claude --help` (2.1.323). The repo is not open source: it holds the changelog, plugins and examples under "All rights reserved" (`LICENSE.md`), with 155k stars and about 17k open issues (`gh api repos/anthropics/claude-code`).

## 1. Architecture and surfaces
- **One binary, many front ends.** The TUI, `-p`, the Agent SDK ("a library that runs the Claude Code binary", docs/agent-sdk/overview), the VS Code/JetBrains extensions, the Desktop "Code" tab, cloud sessions and Slack ("Claude Tag") all share one agent loop (docs/platforms, docs/desktop).
- **Headless.** `-p` takes `--output-format text|json|stream-json`, `--input-format stream-json`, `--include-partial-messages`, `--include-hook-events`, `--forward-subagent-text`, `--replay-user-messages`, `--json-schema`, `--max-budget-usd` and `--permission-prompts host|none` (help).
- **Stream protocol.** The stream opens with `system/init`, which carries a `capabilities` array for feature detection (e.g. `interrupt_receipt_v1`), and ends with `result`. Subagent messages carry `parent_tool_use_id`; retries emit `system/api_retry` (docs/headless). The TypeScript `SDKMessage` union has about 39 variants, including compact boundary, task, rate-limit and permission-denied messages (docs/agent-sdk/typescript).
- **SDK.** TypeScript and Python expose tools, hooks, subagents, MCP, permissions, sessions, plugins, `startup()`/`prewarm()` and control requests (`interrupt`, `set_model`, `applyFlagSettings`). Third parties may not offer claude.ai login (docs/agent-sdk/overview).
- **Background daemon.** `claude --bg`, `attach`, `logs`, `stop`, `respawn`, `rm`, and `claude agents` ("agent view", with `--json`) manage background sessions (help; docs/agent-view).
- **Remote and cloud.** `--remote-control` drives a local session from web or mobile (docs/remote-control). `--cloud` and `--teleport` move sessions to and from Anthropic's cloud or self-hosted runners (docs/claude-code-on-the-web; `claude self-hosted-runner`, CL 2.1.224).
- **Other modes.** `--bare`, `--safe-mode`, `--restricted`, `claude mcp serve`, `claude gateway` (enterprise auth/telemetry) and `claude import` from other agents (help).

## 2. Permissions and sandbox
- **Modes.** `default` ("Manual"), `acceptEdits`, `plan`, `auto`, `dontAsk`, `bypassPermissions`. Deny rules apply in every mode, and critical-path `rm` is never auto-approved (docs/permission-modes).
- **Rules.** `permissions.allow|ask|deny` use `Tool` or `Tool(specifier)`: `Bash(npm run *)`, `Read(~/secrets/**)`, `Edit(/src/**)`, `WebFetch(domain:x)`, `Agent(Explore)`, `Skill(deploy *)`, `mcp__…`, and input-parameter matches such as `Bash(dangerouslyDisableSandbox:true)`.
- **Bash matching** is shell-aware. Compound commands are split on `&& || ; | & \n` and every subcommand must be allowed; deny/ask rules match inside subshells and substitutions. "Don't ask again" saves per-subcommand rules to `.claude/settings.local.json` (docs/permissions).
- **Settings precedence.** Managed > CLI > `.claude/settings.local.json` > `.claude/settings.json` > `~/.claude/settings.json`; lists merge (docs/settings). Managed sources are `/Library/Application Support/ClaudeCode/managed-settings.json`, `/etc/claude-code/managed-settings.json`, the macOS domain `com.anthropic.claudecode`, `HKLM\SOFTWARE\Policies\ClaudeCode`, or server-managed (docs/managed-settings).
- **Auto mode decision order.** Rules first, then auto-approval of reads and in-directory edits, then the classifier for everything else.
  - The classifier is a second model (Sonnet 5 by default, `CLAUDE_CODE_AUTO_MODE_MODEL`) that sees user messages, tool calls and CLAUDE.md, but not tool results.
  - Broad allow rules (`Bash(*)`, interpreters, `Agent`) are dropped on entering auto mode.
  - It can run server-side inside the model request (`CLAUDE_CODE_AUTO_MODE_SERVER`).
  - It reviews subagent spawn prompts, hand-back reports and `SendMessage`.
  - Configuration lives in the `autoMode` settings section (environment, allow, soft_deny, hard_deny) and `claude auto-mode defaults|config|critique` (docs/permission-modes, docs/auto-mode-config, `claude auto-mode --help`).
- **Classifier figures from the engineering post** (2026-03-25): two stages, a single-token filter then chain-of-thought; 0.4% false positives; 17% false negatives on real overeager actions; users accept 93% of manual prompts (https://www.anthropic.com/engineering/claude-code-auto-mode).
- **Bash sandbox.** Seatbelt on macOS; bubblewrap + socat on Linux/WSL2, with an optional seccomp filter for Unix sockets. Native Windows is unsandboxed. The same code ships as `@anthropic-ai/sandbox-runtime`.
  - Network goes through a local proxy with `network.allowedDomains`, which starts empty; in auto mode the classifier approves per-command domains.
  - Filesystem is controlled by `allowWrite`/deny lists and `excludedCommands`.
  - Auto-allow mode skips prompts for sandboxed commands; the `dangerouslyDisableSandbox` retry is an escape hatch.
  - Credential masking gives commands a sentinel value; the proxy substitutes the real secret on egress to `injectHosts`, and re-signs SigV4 (needs `network.tlsTerminate`).
  - Documented limits: no TLS inspection by default, so domain fronting is possible (docs/sandboxing).

## 3. Tools
- **Core list** (docs/tools-reference): Read, Edit, Write, Bash, PowerShell, NotebookEdit, WebFetch, WebSearch, Agent, Skill, Monitor, LSP, ToolSearch, AskUserQuestion, Enter/ExitPlanMode, Enter/ExitWorktree, Cron*, ScheduleWakeup, SendMessage, ListAgents, Workflow, Artifact, Task*, MCP resource tools, PushNotification, EndConversation.
- **Glob and Grep are gone by default** on macOS/Linux; the model uses `find`/`grep` in Bash, which run embedded `bfs`/`ugrep` (CL 2.1.117).
- **Task/Todo tools are omitted** on newer models unless `CLAUDE_CODE_ENABLE_TODO_TOOLS=1`. `TaskOutput` was removed; the model reads the background task's output file with Read (CL 2.1.277).
- **Edit** is exact-string replace with uniqueness and read-before-edit checks. Newer models may skip the read, and a `cat`/`grep` of the file counts as one.
- **Bash** runs one process per command; cwd persists, env does not. Default timeout is 2 minutes (max 10); on timeout the command moves to the background instead of being killed. Output is inlined up to about 30k characters, then spilled to a file with a preview.
- **Monitor** streams lines from a background script, or WebSocket messages, into the conversation as events.
- **WebFetch** is deliberately lossy: the page is summarised by a separate model call, and cross-host redirects are not followed.
- **LSP** needs a code-intelligence plugin and pushes diagnostics after edits.
- **Tool search** defers MCP tool schemas; only names and server instructions load, which also keeps the cache prefix stable (docs/mcp, docs/prompt-caching).
- **Code Mode** (experimental, CL 2.1.323): the model writes JavaScript that calls tools in batch (docs/code-mode).

## 4. Context management
- **Instruction files.** `CLAUDE.md` and `CLAUDE.local.md` load from cwd upward at launch; subdirectory files load lazily when a file there is touched. `@path` imports go up to four hops. `.claude/rules/*.md` take `paths:` globs. `AGENTS.md` is read when no CLAUDE.md exists (CL 2.1.277; docs/memory).
- **Auto memory.** Typed notes live in `~/.claude/projects/<project>/memory/`, shared across worktrees; the first 200 lines or 25KB load each session (docs/memory).
- **Compaction.** Automatic at the limit (about 967K on 1M-context models), manual via `/compact [focus]`, `/autocompact 500k` or `--autocompact`, and partial via `/rewind` → "Summarize from/up to here".
  - Afterwards it re-injects root CLAUDE.md, memory, the plan, skill listings, up to five recently modified files, and invoked skill bodies (5k tokens each, 25k total).
  - Path-scoped rules are lost until re-triggered (docs/context-window, docs/model-config).
  - Idle compaction runs at about 54 minutes, before the 1-hour cache expires (CL 2.1.299).
- **API context editing:** unverified; I found no mention in the docs or changelog.
- **Prompt caching.** The prefix is strictly layered: system and tools, then project context, then conversation.
  - Plan mode, skills and plugin components append as messages so the prefix is not invalidated.
  - CLAUDE.md edits apply only after `/clear` or `/compact`.
  - The system prompt is snapshotted per conversation (`--system-prompt-snapshot`).
  - TTL is 1 hour for the main thread on subscriptions, 5 minutes otherwise (`promptCacheTtl`, `subagentPromptCacheTtl`).
  - `/usage` names the likely cause of a miss.
  - Workflow fan-outs stagger agents by up to 5 seconds so siblings hit the first agent's cache (docs/prompt-caching, docs/workflows).
- **Checkpoints.** File snapshots are taken per user prompt (100 kept); `/rewind` restores code, conversation or both. Bash-made changes and most subagent edits are not tracked (docs/checkpointing).
- **Sessions.** Transcripts are JSONL at `~/.claude/projects/<project>/<session-id>.jsonl`, kept 30 days (`cleanupPeriodDays`). `--resume`, `--continue`, `--fork-session`, `/branch`, `--from-pr` (docs/sessions; help).

## 5. Extensibility
- **Hooks: 33 events** (docs/hooks). SessionStart, Setup, InstructionsLoaded, UserPromptSubmit, UserPromptExpansion, MessageDisplay, PreToolUse, PermissionRequest, PermissionDenied, PostToolUse, PostToolUseFailure, PostToolBatch, Notification, SubagentStart/Stop, TaskCreated/Completed, Stop, StopFailure, TeammateIdle, ConfigChange, CwdChanged, DirectoryAdded, FileChanged, WorktreeCreate/Remove, PreCompact, PostCompact, PreModelSwitch, PostModelSwitch, Elicitation, ElicitationResult, SessionEnd.
  - Handler types: `command`, `http`, `mcp_tool`, `prompt`, `agent`, with an `if` filter in permission-rule syntax.
  - PreToolUse returns `permissionDecision` allow/deny/ask/defer plus `updatedInput`; PostToolUse can return `updatedToolOutput` and `classifierContext`.
  - Stop and others can return `decision:"block"`; exit code 2 blocks.
- **Skills.** `SKILL.md` with frontmatter: `name`, `description`, `when_to_use`, `argument-hint`, `arguments`, `disable-model-invocation`, `user-invocable`, `allowed-tools`, `disallowed-tools`, `model`, `effort`, `context: fork`, `agent`, `background`, `hooks`, `paths`, `shell`. The listing budget is 1% of context, 1,536 characters per skill; `` !`cmd` `` injects command output. Slash commands are merged into skills (docs/skills).
- **Subagents.** `.claude/agents/*.md` with `tools`, `disallowedTools`, `model`, `permissionMode`, `maxTurns`, `skills`, `mcpServers`, `hooks`, `memory`, `background`, `isolation: worktree`, `effort`, `autoCompactWindow`.
  - Built-ins: Explore, Plan, general-purpose, claude, statusline-setup, claude-code-guide.
  - Forks inherit the full conversation and the prompt cache; on by default since CL 2.1.232.
  - Spawns run in the background by default; nesting depth 3; 20 concurrent (docs/sub-agents).
- **Agent teams** (experimental, `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS=1`): peer sessions with a shared task list and direct messaging (docs/agent-teams). Cross-session `SendMessage` is on by default (docs/cross-session-messaging).
- **Workflows.** Deterministic JavaScript with `agent()`, `pipeline()`, `parallel()`, `phase()` and schema-validated outputs. Resumable; `Date.now` and `Math.random` throw; 16 concurrent and 1,000 agents per run. Saved in `.claude/workflows/`; the `ultracode` toggle makes the model plan one per task (docs/workflows).
- **Plugins.** They bundle skills, commands, agents, hooks, MCP and LSP servers, executables, themes, output styles, channels and monitors, distributed through marketplaces. `claude plugin eval` runs scored eval suites (CL 2.1.269).
- **Mods** (CL 2.1.287): in-process JS/TS handlers that draw panes, restyle the UI, intercept tool calls (`tool.check`) and reroute requests (docs/plugins/mods/overview).
- **MCP.** Transports are stdio, HTTP (streamable), SSE (deprecated) and WebSocket. Auth is OAuth (pre-registered clients, `headersHelper`). Also resources, elicitation, server-side tasks, `roots/list`, and scopes local/project (`.mcp.json`)/user (docs/mcp).
- **Other.** Output styles (`keep-coding-instructions`) and a status line script fed JSON (docs/output-styles, docs/statusline).

## 6. Automation
- **In-session.** `/loop`, `CronCreate`, `ScheduleWakeup` (docs/scheduled-tasks). `/goal` has a model check a completion condition after each turn (CL 2.1.139; docs/goal).
- **Cloud "scheduled tasks"** (formerly routines): schedule, API and GitHub triggers (docs/routines).
- **Channels.** MCP servers push events into a live session: Telegram, Discord, iMessage (docs/channels).
- **GitHub.** `claude-code-action` (docs/github-actions); managed Code Review, multi-agent with a verification pass (docs/code-review); `/code-review` locally; `claude ultrareview` in the cloud (help).

## 7. Models
- **Aliases.** `fable`, `opus`, `sonnet`, `haiku`, `best`, `opusplan`, `[1m]` variants.
- **Effort.** `low`…`max`, per-model defaults, saved per model in `modelSettings`; `ultrathink` raises one turn. Thinking is adaptive only on newer models.
- **Fallbacks.** `--fallback-model a,b` / `fallbackModel` (up to 3, per turn, on overload). Separately, content-classifier refusals re-run on an older model.
- **Fast mode.** `/fast`, up to 2.5x speed at premium price, billed as usage credits only (docs/fast-mode).
- **Advisor.** A server-side stronger model the main model can consult (docs/advisor).
- Source for aliases, effort and fallbacks: docs/model-config.

## 8. Shipped in the last six months (2.1.139 on 2026-05-11 → 2.1.323 on 2026-11-12)
- **Models:** Fable 5 (2.1.170), Sonnet 5 (2.1.197), Opus 5 (2.1.219), Fable 5.1 (2.1.257), Opus 5.5 (2.1.280), Sonnet 5.5 (2.1.284), Haiku 5.5 (2.1.293), Fable 5.2 (2.1.321).
- **Agents:** `/goal` (2.1.139); forks on by default (2.1.232); implicit teams (2.1.178); 200-subagent cap removed (2.1.224); self-hosted runners (2.1.224).
- **Safety:** auto mode on Bedrock/Vertex/Foundry (2.1.207); `--restricted` (2.1.248); `--safe-mode` (2.1.169); credential masking (2.1.221); outside-directory read prompt (2.1.257); EndConversation (2.1.214).
- **Extensibility:** MessageDisplay hook (2.1.152); model-switch hooks (2.1.251); Mods (2.1.287); plugin eval (2.1.269); AGENTS.md (2.1.277); Code Mode (2.1.323).
- **Removed:** `/agents` wizard (2.1.198), ultraplan (2.1.222), TaskOutput (2.1.277), MCP v1 client (2.1.323).
- **Pace:** 6,432 changelog bullets since 2.1.140, 4,017 of them "Fixed" (my count over CHANGELOG.md).

## 9. Weaknesses and complaints
From `gh api search/issues`, open issues sorted by comments and by +1 reactions:
- **Usage limits and billing opacity:** #16157 (1,499 comments), #38335 (879), #29579, #97074 (headless said to burn 1.8x quota).
- **TUI rendering:** scroll jumping #826, flicker #769, copy/paste whitespace #18170.
- **Privacy:** #107757, email leaked in a User-Agent header (797 +1 in nine days).
- **Account friction:** phone verification #34229; multi-account #18435.
- **Model behaviour:** rhetorical tics #77136; verbose comments #65961; CLAUDE.md rules ignored #90542.
- **Safeguard false positives:** #84352, #87640. Classifier no-verdict outages block all Bash: #97854.
- **Feature interactions:** auto mode's Bash-first instruction stops nested CLAUDE.md and path rules loading (#90450); compound-command prompts (#28240).
- **Reliability:** stream idle timeouts #46987, hangs #26224, memory growth #4953.
- **Platform gaps:** XDG non-compliance #1455; Windows/Desktop breakage #92984, #85199.

## Techniques worth copying
- A cache-first prompt layout: append-only injections, a system-prompt snapshot, deferred tool schemas, forks that share the parent's cache, staggered fan-out, and miss-cause diagnostics.
- The classifier design: it never sees tool results, drops broad allow rules, and reviews subagent spawns and hand-backs, with a deterministic rule layer above it.
- An OS sandbox with a proxy allowlist and sentinel-credential substitution; sandboxed commands are auto-allowed.
- Timeout moves a command to the background; oversized output goes to a file with a preview; Monitor delivers events.
- One permission-rule grammar reused across settings, hooks (`if`), skills and subagents.
- Dropping Glob/Grep/Todo tools in favour of the shell, with embedded fast binaries.
- Deterministic, resumable workflow scripts with schema-typed agent outputs.
- A `capabilities` array in `system/init` for protocol feature detection.
- An explicit table of what survives compaction, and partial summarisation from a rewind point.

## Weaknesses a competitor could exploit
- Closed source, Anthropic-only models, and many features tied to claude.ai login (channels, artifacts, routines, push).
- Opaque quota accounting and cache-TTL-dependent cost.
- No native Windows sandbox; Linux needs extra packages and AppArmor workarounds.
- Sprawl: 33 hook events, six overlapping orchestration mechanisms (subagents, forks, teams, workflows, agent view, cross-session messaging), hundreds of env vars, and a 63% bug-fix changelog.
- A server-side classifier is a single point of failure and a privacy concern; safeguard-driven model fallbacks surprise users.
- Checkpoints ignore Bash and subagent edits; a git-native snapshot would beat this.
- TUI flicker and scroll bugs open since 2025; a Rust renderer is a direct selling point.
- Non-XDG config in `~/.claude`, and plaintext JSONL transcripts that include system prompts.

