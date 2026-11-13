# Coding-agent harness market, excluding Codex CLI and Claude Code (2026-11-14)

Web search and fetch worked. All stats are from `gh` today. Feature claims cite a repo path (relative to the row's repo) or a URL; anything I could not confirm is marked "unverified".

## 1. Verified stats

| Project | Repo | Stars | Lang | License | Latest release | Last push |
|---|---|---|---|---|---|---|
| DeepSeek Harness | https://github.com/deepseek-ai/deepseek-harness | 280,408 | TS | MIT | none (created 2026-08-13) | 11-13 |
| OpenCode | https://github.com/anomalyco/opencode (was sst/) | 226,625 | TS | MIT | v1.19.5, 10-28 | 11-13 |
| Pi | https://github.com/earendil-works/pi (was badlogic/pi-mono) | 121,845 | TS | MIT | v1.5.2, 11-10 | 11-13 |
| Gemini CLI | https://github.com/google-gemini/gemini-cli | 108,815 | TS | Apache-2.0 | v0.68.0, 11-10 | 11-13 |
| Zed (editor and agent) | https://github.com/zed-industries/zed | 94,937 | Rust | mixed (gh: NOASSERTION) | v1.28.1, 11-11 | 11-13 |
| OpenHands | https://github.com/OpenHands/OpenHands (was All-Hands-AI/) | 93,411 | TS | MIT | v1.40.0, 11-12 | 11-13 |
| Cline | https://github.com/cline/cline | 71,588 | TS | Apache-2.0 | v4.1.26, 10-26 | 11-13 |
| Warp | https://github.com/warpdotdev/warp | 66,007 | Rust | AGPL-3.0 (UI crates MIT) | 2026-06-03 stable | 11-13 |
| Goose | https://github.com/aaif-goose/goose (was block/) | 56,538 | Rust | Apache-2.0 | v1.59.0, 11-10 | 11-13 |
| Aider | https://github.com/Aider-AI/aider | 50,109 | Python | Apache-2.0 | v0.86.0, 2025-08-09 | 2026-05-22 |
| oh-my-pi | https://github.com/can1357/oh-my-pi | 38,946 | TS + Rust core | MIT | v18.20.0, 11-12 | 11-13 |
| Qwen Code | https://github.com/QwenLM/qwen-code | 30,108 | TS | Apache-2.0 | sdk-ts v0.1.33, 11-13 | 11-13 |
| Crush | https://github.com/charmbracelet/crush | 29,075 | Go | FSL-1.1-MIT | v0.101.0, 11-12 | 11-13 |
| Kilo Code | https://github.com/Kilo-Org/kilocode | 27,976 | TS | MIT | v7.8.27, 11-05 | 11-13 |
| Grok Build | https://github.com/xai-org/grok-build | 27,761 | Rust | Apache-2.0 | none (binaries via x.ai) | 09-29 |
| Copilot CLI | https://github.com/github/copilot-cli | 11,410 | closed (release repo) | proprietary | v1.0.111, 11-13 | 11-13 |
| mini-swe-agent | https://github.com/SWE-agent/mini-swe-agent | 9,590 | Python | MIT | v2.4.6, 07-23 | 11-09 |
| Kimi Code | https://github.com/MoonshotAI/kimi-code | 8,215 | TS | MIT | not checked | 11-13 |
| ForgeCode | https://github.com/tailcallhq/forgecode | 7,631 | Rust | Apache-2.0 | v2.14.0, 10-01 | 11-13 |
| Mistral Vibe | https://github.com/mistralai/mistral-vibe | 5,383 | Python | Apache-2.0 | v3.1.2, 11-10 | 11-10 |
| Kiro | https://github.com/kirodotdev/Kiro (issue tracker only) | 4,466 | closed | n/a | n/a | 11-05 |
| Junie CLI | https://github.com/JetBrains/junie (installer shim) | 558 | closed | proprietary | 4128.1, 11-13 | 11-13 |
| Auggie | https://github.com/augmentcode/auggie (shim) | 291 | closed | proprietary | v0.28.0, 05-22 | 11-13 |
| Factory Droid | https://github.com/Factory-AI/factory (shim) | 89 | closed | n/a | none | 11-11 |

Amp, Cursor CLI and Devin have no public source repo (`sourcegraph/amp` and `cursor/cli` do not resolve).

**Dead or fading; do not benchmark against these:**
- **Roo Code:** archived, last push 2026-05-15 (24,237 stars). Secondary sources say the team moved to a cloud agent.
- **Continue:** the README says "no longer actively maintained and is read-only"; final 2.0.0 release.
- **Aider:** no release in 15 months.
- **Plandex:** last commit 2025-10-03, "link to cloud wind down post".
- **SWE-agent:** its README says mini-swe-agent "has superseded" it.
- **Trae agent:** last push 2026-02-05.
- **superagent-ai/grok-cli:** stale since July; the official Grok Build now exists.
- **MoonshotAI/kimi-cli:** archived in favour of kimi-code.

**Adjacent, not coding-first:** openclaw/openclaw (394k stars, built on Pi per Pi's README) and NousResearch/hermes-agent (263k).

## 2. Distinguishing features

**DeepSeek Harness (`dsh`)**
- "Everything is a plugin" on the Cordis framework: model adapter, tool registry, session log and the agent loop itself are replaceable plugins with reversible effects and hot reload (`docs/architecture.md`).
- A running harness is composed from profiles (`web`, `headless`, `sdk`, `sdk-minimal`, `acp`) plus ordered YAML patches; `dsh --dump-config` prints the tree.
- Append-only session event log with the invariant "model-visible means logged"; fork, resume and telemetry derive from it, and the format has versioned migrations.
- The sandbox is a swappable "seam": Linux bwrap/Landlock, macOS Seatbelt, Windows ACL restricted token. It reports `full` or `partial` enforcement as a fact. Swapping the fs/subprocess provider to SSH moves Bash, PTY and LSP with it (`docs/subsystems/sandbox.md`).
- Tools include `apply_patch`, `edit`, `str_replace_editor`, `lsp`, `run_code`, `workflow`, `ralph`, team task board, `schedule_*` and `session_search` (`docs/tool-catalog.md`).
- It is a developer preview with no security audit (`SAFETY.md`). Which model providers it supports is unverified.

**OpenCode (v2)**
- Server-first: plugins have a server part, a TUI part and a GUI part in one package (`services/site/src/docs/content/build/plugins/index.mdx`); `opencode acp`; web via `opencode pair`.
- Ordered permission rules of `{action, resource, effect}`, last match wins (`permissions.mdx`). Separate binary "policies" that only tighten and can be pushed by an organisation (`policies.mdx`).
- Snapshots: `/undo` stages a rollback of files and conversation and returns the prompt to the composer (`snapshots.mdx`).
- Compaction keeps about 15k recent tokens and reuses the prompt cache (`compaction.mdx`). "Warming" pings the provider to keep the cache alive (`warming.mdx`).
- Can drive Claude Code as a provider (`claude-agent.mdx`).
- No OS sandbox page found in the v2 docs (unverified).

**Pi**
- Deliberately minimal: "skips features like sub-agents and plan mode", and has no built-in permission system (README).
- Session is a JSONL tree with branches and branch summarisation (`packages/coding-agent/docs/how-pi-works.md`, `compaction.md`).
- In-process TypeScript extensions register tools, providers, UI and compaction (`extensions.md`).
- `codemode`: the model writes JavaScript in a QuickJS sandbox that calls tools; only the script output reaches context (`codemode.md`).
- Virtual models: a router maps a selected model to a physical one per request (`virtual-models.md`).
- Isolation is delegated to a Gondolin micro-VM extension, Docker or OpenShell. MCP is now supported (`mcp.md`).

**oh-my-pi** (Pi fork; all from README)
- Hashline edits: the model points at content-hash anchors instead of retyping lines; stale anchors are rejected.
- LSP on every write (14 operations) and a real debugger over DAP (28 operations).
- "Time-traveling stream rules": a regex aborts the stream mid-token, injects a rule and retries.
- An "advisor" second model reads every turn.
- Subagents fan out into copy-on-write isolated workspaces (apfs, btrfs, overlayfs) and return schema-validated results.
- Virtual schemes such as `pr://` and `agent://` work inside the file tools.

**Gemini CLI**
- Widest sandbox menu: Seatbelt, Docker/Podman, Windows native, gVisor, LXC (`docs/cli/sandbox.md`).
- Checkpoints in a shadow git repository under `~/.gemini/history` (`docs/cli/checkpointing.md`).
- Remote subagents over A2A (`docs/core/remote-agents.md`).
- Auto Memory mines past transcripts into reviewable memory patches and `SKILL.md` drafts (`docs/cli/auto-memory.md`).
- A local Gemma model can make routing decisions (`docs/core/local-model-routing.md`).
- Gemini models only.

**Qwen Code**
- Gemini CLI fork, independent since v0.1; multi-protocol (OpenAI, Anthropic, Gemini, Qwen).
- `qwen serve` daemon over HTTP and SSE with multi-client shared sessions.
- IM channels (Telegram, DingTalk, WeChat, Feishu); SDKs in TypeScript, Python and Java; Agent Teams. All from the README.

**Goose**
- Rust; now governed by the Linux Foundation's Agentic AI Foundation (README).
- Uses other agents as model backends over ACP, so users can spend Claude or ChatGPT subscriptions (https://goose-docs.ai/docs/guides/acp-providers).
- Adversary mode: an independent reviewer returns ALLOW or BLOCK per tool call, and fails open (`documentation/docs/guides/security/adversary-mode.md`).
- Recipes and subrecipes, code mode for MCP, custom branded distributions (`CUSTOM_DISTROS.md`).
- OS sandbox unverified.

**Cline**
- Re-platformed as an SDK that powers the CLI, a Tauri desktop app, VS Code and JetBrains (README).
- Shadow-git checkpoints (`docs/core-workflows/checkpoints.mdx`).
- Multi-agent teams with persisted state, and cron-scheduled agents (README).
- Kanban board running parallel agents in worktrees (`docs/usage/kanban.mdx`).

**Kilo Code**
- The CLI core lives in `packages/opencode`, so it appears to be an OpenCode fork (`AGENTS.md`).
- Adds VS Code and JetBrains clients, a cloud agent, a marketplace and a 500-model gateway at zero markup (README).

**Crush**
- Go. `crush serve` groups clients into workspaces that share sessions, permission queue, LSP and MCP state (README).
- 13 hook events, Claude Code-compatible payloads; hooks run in parallel and compose in config order (`docs/hooks/README.md`).
- The bash tool parses commands and auto-allows only provably inert ones (README).
- Licence is FSL, not OSI open source.

**Grok Build**
- Rust TUI. Kernel sandbox profiles `workspace`, `read-only`, `strict` (Landlock, seccomp, Seatbelt, bubblewrap); it write-protects its own config and hook directories (`crates/codegen/xai-grok-pager/docs/user-guide/18-sandbox.md`).
- Permission modes use Claude-compatible names, plus an "auto" safety check (`22-permissions-and-safety.md`).
- Personas overlay subagents (`16-subagents.md`).
- `grok clone` mounts a projected working tree over FUSE or NFS (`27-grok-clone.md`).
- External contributions are not accepted.

**Copilot CLI**
- `/sandbox`, "assisted permissions" where a model approves requests, `--allow-tool`/`--deny-tool`, Copilot Memory, ACP server, BYO provider via `COPILOT_PROVIDER_*` (https://docs.github.com/en/copilot/concepts/agents/about-copilot-cli).
- The changelog shows `/rewind`, `/lsp`, plugin marketplaces, `/worktree`, `/every` schedules and remote control from GitHub (`changelog.md`).

**Cursor CLI**
- Run modes: allowlist, sandbox, then a backend classifier that can read files to judge a call. The sandbox is Seatbelt on macOS and Landlock plus seccomp on Linux, with a domain allowlist in `sandbox.json` (https://cursor.com/docs/agent/security/run-modes).
- `&` hands the conversation to a cloud agent; sudo passwords bypass the model (https://cursor.com/docs/cli/overview).
- ACP, hooks, subagents, skills, plugins and an SDK have doc pages (https://cursor.com/llms.txt).

**Amp**
- "Orbs" are per-thread cloud machines; `--executor local|orb|runner:<id>` (https://ampcode.com/manual, https://ampcode.com/docs/cli).
- Oracle, Librarian and Finder are now installable plugins. The plugin API intercepts `tool.call` with allow, reject, modify or synthesize (https://ampcode.com/docs/customize/plugins).
- Stated policy: "No backward compatibility".

**Factory Droid**
- Autonomy levels Off/Low/Medium/High matched against a per-command risk rating; Spec Mode; Missions (multi-agent) (https://docs.factory.com/cli/user-guides/auto-run).
- ACP for JetBrains and Zed (README).

**Kiro**
- One harness as a standalone server replaced three (TypeScript IDE, Rust CLI, Python web); ACP is the client boundary, with `_kiro/` extensions.
- Cedar-backed capability permissions, deny always wins (https://kiro.dev/blog/one-agent/).
- Specs with property-based tests (README).

**Zed**
- Origin of ACP; hosts external agents. Its native agent has an OS sandbox, tool permissions, parallel threads in worktrees and per-hunk review (`docs/src/ai/sandboxing.md`, https://zed.dev/docs/ai/agent-panel).

**Warp**
- Client open-sourced under AGPL, with OpenAI as founding sponsor; hosts third-party CLI agents; "Factories" are cloud agent pipelines (README, https://docs.warp.dev/agents/overview).

**Mistral Vibe**
- Hooks (`pre_agent`, `session_start` and others), a versioned skills registry, `/vibe:import` from other agents, hard `--max-price` stops in headless mode (`CHANGELOG.md`).

**Junie CLI**
- BYOK, OS sandbox in early access, debug mode against a live JetBrains debugger (https://junie.jetbrains.com/docs/junie-cli.html).

**mini-swe-agent**
- About 100 lines, bash only, linear history, stateless `subprocess.run`; the research baseline (README).

**Devin**
- Playbooks are converting to Skills; Memory with nightly "dreaming"; child sessions start from a parent summary (https://docs.devin.ai/release-notes/overview).

**OpenHands**
- Pivoted to "Agent Canvas", a control centre running any ACP agent across local, Docker, VM and cloud backends (README).

## 3. Feature matrix

"?" means not verified.

| | OpenCode | DeepSeek Harness | Pi | Gemini CLI | Cline | Goose | Copilot CLI | Cursor CLI | Grok Build | Crush |
|---|---|---|---|---|---|---|---|---|---|---|
| OS sandbox | ? | yes | no (external) | yes | ? | ? | yes | yes | yes | ? |
| Approvals | rules + policies | yes | no | yes | yes | modes + adversary | yes + model-assisted | classifier | modes + rules | yes |
| Checkpoints/undo | yes | ? | conversation tree | yes | yes | ? | yes | ? | yes | ? |
| Subagents | yes | yes + teams | no | yes + A2A | yes + teams | yes | yes | yes | yes | ? |
| MCP | yes | yes | yes | yes | yes | yes | yes | ? | yes | yes |
| Hooks | via plugins | plugin events | extension events | yes | yes | yes | yes | yes | yes | yes |
| Skills/plugins | both | both | skills + packages | skills + extensions | both | both | both | both | both | skills |
| Client-server / ACP | both | JSON-RPC + ACP | RPC; ACP via adapter | ACP | SDK + ACP | API + ACP | ACP | ACP | ACP | server |
| LSP | yes | yes | no | ? | ? | ? | yes | ? | ? | yes |
| Repo map/index | ? | ? | no | ? | ? | ? | ? | ? | ? | ? |
| Headless/SDK | yes | yes | yes | yes | yes | yes | yes | yes | yes | ? |
| Multi-provider | yes | ? | yes | no | yes | yes | BYOK | ? | custom models | yes |

## 4. Ideas only one or two tools have
- Agent loop as a hot-swappable plugin with reversible effects (DeepSeek Harness).
- Sandbox that reports full versus partial enforcement (DeepSeek Harness).
- Hash-anchored edits (oh-my-pi).
- Mid-stream regex abort, inject and retry (oh-my-pi).
- Second-model reviewer per turn or per tool call (oh-my-pi advisor, Goose adversary).
- Model or classifier as approver (Cursor, Copilot).
- Scripted tool calling where only the output enters context (Pi, Goose; DeepSeek `run_code`).
- Other agents as model backends over ACP (Goose, OpenCode).
- Cache warming (OpenCode).
- Virtual model routers (Pi; Gemini with local Gemma).
- Debugger over DAP (oh-my-pi, Junie).
- Copy-on-write or projected workspaces (oh-my-pi, Grok).
- Shared multi-client workspace with one permission queue (Crush, Qwen).
- Cedar policies (Kiro).
- Transcript mining into skills (Gemini, Qwen).
- Property-based tests generated from specs (Kiro).

## 5. Table stakes in late 2026
- MCP, including OAuth.
- `AGENTS.md` and Agent Skills (`SKILL.md`).
- Lifecycle hooks, usually Claude-compatible.
- Subagents with isolated context.
- Plan mode.
- Allow/ask/deny rules.
- Auto-compaction.
- Headless `-p` with JSON output.
- ACP server mode.
- Session resume and fork.
- Plugin bundles and marketplaces.
- Git worktrees for parallel work.
- Importing other agents' configuration.

## 6. Where the market is moving
1. **Harness as server, UIs as clients.** Kiro merged three harnesses into one ACP server; OpenCode v2, `crush serve`, `qwen serve` and the DeepSeek profiles do the same.
2. **ACP won the editor boundary.** 51 agents are listed at https://agentclientprotocol.com/get-started/agents; OpenHands and Warp now host other vendors' agents.
3. **Kernel sandboxes plus model-judged approvals replace prompt fatigue.** Cursor, Grok Build, Copilot, Zed, Junie and DeepSeek Harness all ship one or both.
4. **Minimal, extensible cores beat feature-heavy ones.** Pi (122k stars) and DeepSeek Harness (280k in three months) lead; Aider, Roo, Continue and Plandex faded.
5. **Local to cloud continuity and fleets.** Amp orbs, Cursor's `&` handoff, Kiro cloud sessions, Cline Kanban and Warp Factories.
6. **Rust and single binaries for vendor CLIs.** Grok Build, Goose, Warp, the oh-my-pi core and ForgeCode are Rust; Kimi Code advertises a single binary with no Node.js required.

Nothing on this machine was modified apart from scratch files in the session scratchpad.
