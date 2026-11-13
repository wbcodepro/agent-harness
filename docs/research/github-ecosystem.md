# Coding-agent support ecosystem on GitHub (as of 2026-11-14)

**Method.** About 75 `gh search repos` queries (with and without `created:>2026-01-01`), 12 topic queries, GraphQL metadata for about 190 repos, and READMEs for about 80. Stars are `gh` values from today; C = created, P = last push (2026 unless a year is given). I did no web search; protocol facts come from the spec repos' release tags and changelog.

**Caveats.**
- Star counts across this space are extremely high (seven repos created in 2026 exceed 140k). I could not check star authenticity, so treat stars as relative attention; where shown, commit and fork counts are the substance check.
- All benchmark and savings figures quoted from READMEs are vendor self-reported and unverified.

**Scale of activity.** Repos created since 2026-03-01 by topic:

| Topic | Repos |
|---|---|
| `claude-code` | 107,955 |
| `mcp` | 99,807 |
| `codex` | 48,175 |
| `agent-skills` | 40,887 |
| `mcp-server` | 33,535 |
| `claude-code-plugin` | 14,592 |
| `coding-agent` | 7,867 |
| `git-worktree` | 1,735 |
| `acp` | 968 |
| `agent-sandbox` | 168 |

`agent-skills` has 42,875 repos in total, of which 42,275 were created in 2026.

## Categories

### (a) Orchestration and parallelism
| Repo | Stars | C / P | What it does; key technique |
|---|---|---|---|
| [stablyai/orca](https://github.com/stablyai/orca) | 124.3k | 03-17 / 11-13 | Desktop and mobile fleet runner. One git worktree per agent; an orchestrator agent splits work to workers; runs over SSH or cloud VMs (21k commits). |
| [paperclipai/paperclip](https://github.com/paperclipai/paperclip) | 113.6k | 03-02 / 11-13 | "Company" of agents on a Node server with React UI. Org chart, goals, per-agent budgets and governance. |
| [herdrdev/herdr](https://github.com/herdrdev/herdr) | 50.8k | 03-27 / 11-07 | Rust terminal runtime. A background server keeps agent terminals alive; panes are marked working/blocked/idle; agents drive it through a CLI and socket API. |
| [Yeachan-Heo/oh-my-claudecode](https://github.com/Yeachan-Heo/oh-my-claudecode) | 40.8k | 01-09 / 11-13 | Multi-agent "teams" layer installed on top of Claude Code. |
| [BloopAI/vibe-kanban](https://github.com/BloopAI/vibe-kanban) | 28.7k | 2025-06-14 / 11-12 | Kanban issue to workspace (branch, terminal, dev server) to diff review. Rust. |
| [gastownhall/gastown](https://github.com/gastownhall/gastown) | 18.8k | 2025-12-16 / 11-12 | Workspace for 20–30 agents. Mailboxes and identities; work state persisted in git-backed hooks and the Beads ledger. |

Also notable: [openai/symphony](https://github.com/openai/symphony) (28.0k, 47 commits) is mainly a `SPEC.md` ("tell your agent to implement it") plus an Elixir reference: Linear board to isolated runs to "proof of work".

### (b) Sandboxes and execution environments
| Repo | Stars | C / P | What it does; key technique |
|---|---|---|---|
| [opensandbox-group/OpenSandbox](https://github.com/opensandbox-group/OpenSandbox) | 17.3k | 2025-12-17 / 11-13 | Unified sandbox API, Docker locally and Kubernetes in production, with network access control. |
| [NVIDIA/NemoClaw](https://github.com/NVIDIA/NemoClaw) | 23.0k | 03-15 / 11-13 | Reference stack running agents in OpenShell sandboxes with network policy and managed inference. |
| [anthropics/sandbox-runtime](https://github.com/anthropics/sandbox-runtime) | 7.1k | 2025-10-20 / 11-13 | Container-free OS sandbox: `sandbox-exec` on macOS, `bubblewrap` on Linux, plus a proxy-based network allowlist. |
| [deeplethe/sandstorm](https://github.com/deeplethe/sandstorm) | 6.1k | 05-11 / 11-01 | Rust. gVisor or Firecracker; idle sandboxes are paused and snapshotted to disk, then restored on request. |
| [nolabs-ai/nono](https://github.com/nolabs-ai/nono) | 5.0k | 01-31 / 11-13 | Rust single binary, unprivileged kernel-enforced sandbox, plus credential protection, audit and recovery. |
| [earendil-works/gondolin](https://github.com/earendil-works/gondolin) | 2.4k | 02-03 / 11-13 | Local micro-VM (libkrun or QEMU). The guest sees placeholder secrets; the host injects real ones only for allowed hosts. |

Older infrastructure: [e2b-dev/E2B](https://github.com/e2b-dev/E2B) (14.8k) is the API others clone ("E2B-compatible"). [daytonaio/daytona](https://github.com/daytonaio/daytona) (71.3k) is now archived.

### (c) Memory and context
| Repo | Stars | C / P | What it does; key technique |
|---|---|---|---|
| [Graphify-Labs/graphify](https://github.com/Graphify-Labs/graphify) | 147.2k | 04-03 / 11-12 | Codebase to knowledge graph. Deterministic tree-sitter AST pass with no LLM; edges tagged EXTRACTED or INFERRED. |
| [JuliusBrussee/caveman](https://github.com/JuliusBrussee/caveman) | 119.7k | 04-04 / 11-13 | Terse-output skill plus proxy to cut tokens. |
| [thedotmack/claude-mem](https://github.com/thedotmack/claude-mem) | 116.8k | 2025-08-31 / 11-13 | A second "note-taker" agent; the next session starts from a short index and pulls details on demand. |
| [rtk-ai/rtk](https://github.com/rtk-ai/rtk) | 86.5k | 01-22 / 11-13 | Rust CLI proxy that compresses output of 100+ dev commands (tests, diffs, logs) in under 25 ms. |
| [headroomlabs-ai/headroom](https://github.com/headroomlabs-ai/headroom) | 79.6k | 01-07 / 11-13 | Local compression of tool outputs, logs and RAG chunks before the LLM call. |
| [colbymchenry/codegraph](https://github.com/colbymchenry/codegraph) | 77.5k | 01-18 / 11-01 | Pre-indexed local code graph with a Rust kernel, auto-synced on change. |

Also:
- Context: [upstash/context7](https://github.com/upstash/context7) (64.8k, docs fetcher) and [mksglu/context-mode](https://github.com/mksglu/context-mode) (34.0k, keeps tool output out of context in a sandbox with SQLite).
- Memory: [MemPalace/mempalace](https://github.com/MemPalace/mempalace) (60.5k, verbatim storage, no summarising) and [gastownhall/beads](https://github.com/gastownhall/beads) (28.7k, dependency-graph issue tracker used as agent memory).
- Code search: [MinishLab/semble](https://github.com/MinishLab/semble) (6.4k, CPU-only).

### (d) Skills, plugins, hooks
| Repo | Stars | C / P | What it does; key technique |
|---|---|---|---|
| [obra/superpowers](https://github.com/obra/superpowers) | 317.2k | 2025-10-09 / 11-03 | Composable skills forming a methodology: brainstorm, spec, plan, subagent implementation with red/green TDD. |
| [affaan-m/ECC](https://github.com/affaan-m/ECC) | 302.4k | 01-18 / 11-09 | Skills, memory and security bundle across 7 harnesses; a paid GitHub App is upsold. |
| [anthropics/skills](https://github.com/anthropics/skills) | 194.1k | 2025-09-22 / 11-06 | Reference skills repository. |
| [DietrichGebert/ponytail](https://github.com/DietrichGebert/ponytail) | 190.2k | 06-12 / 10-29 | A "write less code" behaviour skill. |
| [addyosmani/agent-skills](https://github.com/addyosmani/agent-skills) | 117.1k | 02-15 / 11-01 | Engineering-practice skills. |
| [vercel-labs/skills](https://github.com/vercel-labs/skills) | 39.2k | 01-14 / 11-12 | `npx skills add/find`: a cross-agent installer targeting about 80 agents. |

Marketplace: [anthropics/claude-plugins-official](https://github.com/anthropics/claude-plugins-official) (47.2k). Hooks-only collections are tiny (top "agent hooks" result: 468 stars); hooks ship inside other tools.

### (e) Spec-driven and workflow frameworks
| Repo | Stars | C / P | What it does; key technique |
|---|---|---|---|
| [github/spec-kit](https://github.com/github/spec-kit) | 149.7k | 2025-08-21 / 11-12 | Templates and processes (spec-driven development, bug fixing, idea assessment) as reusable primitives. |
| [garrytan/gstack](https://github.com/garrytan/gstack) | 141.4k | 03-11 / 11-13 | Role-based slash commands (CEO, eng manager, QA with a real browser, security, release). |
| [Fission-AI/OpenSpec](https://github.com/Fission-AI/OpenSpec) | 74.8k | 2025-08-05 / 11-13 | Per-change folder (`proposal.md`, `specs/`, `design.md`, `tasks.md`), archived on completion; aimed at brownfield code. |
| [gsd-build/get-shit-done](https://github.com/gsd-build/get-shit-done) | 64.1k | 2025-12-14 / 05-31 | Meta-prompting plus spec-driven development; stale since May. |
| [bmad-code-org/BMAD-METHOD](https://github.com/bmad-code-org/BMAD-METHOD) | 55.3k | 2025-04-13 / 11-13 | Agile role-agent method. |
| [OthmanAdi/planning-with-files](https://github.com/OthmanAdi/planning-with-files) | 28.0k | 01-03 / 11-11 | `task_plan.md`, `findings.md` and `progress.md` on disk, re-injected by lifecycle hooks; survives `/clear` and compaction. |

### (f) MCP servers and tooling
| Repo | Stars | C / P | What it does; key technique |
|---|---|---|---|
| [ChromeDevTools/chrome-devtools-mcp](https://github.com/ChromeDevTools/chrome-devtools-mcp) | 55.4k | 2025-09-11 / 11-13 | DevTools (console, network, performance) for agents. |
| [vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser) | 50.6k | 01-11 / 11-13 | Rust CLI, not MCP. Accessibility-tree snapshot with `@e1` refs, one shell call per step. |
| [microsoft/playwright-mcp](https://github.com/microsoft/playwright-mcp) | 39.6k | 2025-03-21 / 11-13 | Browser automation over MCP. |
| [github/github-mcp-server](https://github.com/github/github-mcp-server) | 34.4k | 2025-03-04 / 11-13 | Official GitHub MCP server. |
| [oraios/serena](https://github.com/oraios/serena) | 31.4k | 2025-03-23 / 11-09 | Semantic retrieval and editing toolkit. |
| [modelcontextprotocol/registry](https://github.com/modelcontextprotocol/registry) | 7.5k | 2025-02-05 / 11-13 | Community registry service. |

Gateways are small: [maximhq/bifrost](https://github.com/maximhq/bifrost) 9.5k, [agentgateway/agentgateway](https://github.com/agentgateway/agentgateway) 6.2k (Rust; MCP and A2A proxy). "Code mode" gateways that collapse N tools into search plus execute have under 200 stars each. The CLI-plus-skill pattern ([vercel-labs/agent-desktop](https://github.com/vercel-labs/agent-desktop) 23.2k, [googleworkspace/cli](https://github.com/googleworkspace/cli) 32.3k) is visibly competing with MCP.

### (g) Evaluation and benchmarks
| Repo | Stars | C / P | What it does; key technique |
|---|---|---|---|
| [microsoft/SkillOpt](https://github.com/microsoft/SkillOpt) | 19.1k | 05-08 / 11-10 | Trains skills in text space with epochs, batches and validation gates. |
| [harbor-framework/harbor](https://github.com/harbor-framework/harbor) | 7.0k | 2025-08-04 / 11-13 | Official Terminal-Bench 2.0 harness. Runs any agent in Docker or cloud sandboxes in parallel, with adapters for SWE-bench and others. |
| [SWE-bench/SWE-bench](https://github.com/SWE-bench/SWE-bench) | 6.2k | 2023-10-04 / 09-18 | The standard issue-resolution benchmark. |
| [harbor-framework/terminal-bench](https://github.com/harbor-framework/terminal-bench) | 1.0k | 01-25 / 11-13 | Current Terminal-Bench repo (v1 at [terminal-bench-1](https://github.com/harbor-framework/terminal-bench-1), 2.6k). |
| [china-qijizhifeng/agentic-harness-engineering](https://github.com/china-qijizhifeng/agentic-harness-engineering) | 969 | 04-23 / 10-30 | Observability-driven automatic harness evolution. |
| [UiPath/coder_eval](https://github.com/UiPath/coder_eval) | 160 | 07-09 / 11-13 | Declarative test file for skills, MCP servers and CLIs, with a CI gate. |

### (h) Observability, cost, session viewers
| Repo | Stars | C / P | What it does; key technique |
|---|---|---|---|
| [jarrodwatts/claude-hud](https://github.com/jarrodwatts/claude-hud) | 29.2k | 01-02 / 11-10 | Status-line HUD: context %, rate limits, tools, subagents, todos. |
| [ccusage/ccusage](https://github.com/ccusage/ccusage) | 19.4k | 2025-05-29 / 11-13 | Cost reports parsed from local session logs of 7+ CLIs (now Rust). |
| [FailproofAI/failproofai](https://github.com/FailproofAI/failproofai) | 8.8k | 04-06 / 11-13 | Hooks into 12 harnesses; 40 policies that block tool calls, plus session history. |
| [matt1398/claude-devtools](https://github.com/matt1398/claude-devtools) | 4.0k | 02-07 / 09-26 | Transcript inspector, built because the CLI collapsed tool detail. |
| [graykode/abtop](https://github.com/graykode/abtop) | 3.9k | 03-29 / 09-14 | Rust "btop for agents": sessions, context %, orphan ports. |
| [liaohch3/claude-tap](https://github.com/liaohch3/claude-tap) | 3.3k | 02-15 / 11-13 | Local proxy that shows real API traffic and request diffs. |

Generic: [langfuse/langfuse](https://github.com/langfuse/langfuse) (37.4k).

### (i) Code review and CI
| Repo | Stars | C / P | What it does; key technique |
|---|---|---|---|
| [alibaba/open-code-review](https://github.com/alibaba/open-code-review) | 50.4k | 05-18 / 11-13 | Go CLI. A tool-using agent reads the diff plus the full repo and emits line-level structured comments. |
| [The-PR-Agent/pr-agent](https://github.com/The-PR-Agent/pr-agent) | 16.1k | 2023-07-05 / 11-13 | The original open-source PR reviewer. |
| [anthropics/claude-code-action](https://github.com/anthropics/claude-code-action) | 9.9k | 2025-05-19 / 11-13 | Agent as a GitHub Action. |
| [openai/codex-action](https://github.com/openai/codex-action) | 1.3k | 2025-10-01 / 10-31 | Same for Codex. |
| [taovc/pr-cockpit](https://github.com/taovc/pr-cockpit) | 486 | 05-29 / 10-27 | Local review in worktrees, human-gated posting. |

This category is thin outside vendors.

### (j) Protocols and standards
| Repo | Stars | Status (from `gh`) |
|---|---|---|
| [modelcontextprotocol/modelcontextprotocol](https://github.com/modelcontextprotocol/modelcontextprotocol) | 9.7k | Spec revision `2026-07-28`. Stateless: no `initialize` handshake or session header, new `server/discover`, tasks moved to an extension, server-initiated requests replaced by `input_required` results. |
| [agentclientprotocol/agent-client-protocol](https://github.com/agentclientprotocol/agent-client-protocol) | 4.6k | Rust crate v1.17.0, schema v1.31.0, schema v2.0.0-alpha.15 (all 11-10). The [registry](https://github.com/agentclientprotocol/registry) has about 49 top-level directories. |
| [agentskills/agentskills](https://github.com/agentskills/agentskills) | 27.3k | `SKILL.md` folder spec; no tagged releases. |
| [agentsmd/agents.md](https://github.com/agentsmd/agents.md) | 25.5k | Plain-markdown instruction file. |
| [a2aproject/A2A](https://github.com/a2aproject/A2A) | 26.5k | v1.0.1 (05-28). |
| [google-labs-code/design.md](https://github.com/google-labs-code/design.md) | 28.9k | C 04-10; design tokens in YAML plus prose. |

### (k) UIs and remote control
| Repo | Stars | C / P | What it does; key technique |
|---|---|---|---|
| [farion1231/cc-switch](https://github.com/farion1231/cc-switch) | 154.2k | 2025-08-04 / 11-13 | Rust desktop app. Switches providers and manages MCP, skills and prompts across 12 CLIs. |
| [pingdotgg/t3code](https://github.com/pingdotgg/t3code) | 59.4k | 02-08 / 11-13 | One server with desktop, web and phone clients. Drives installed agents and any ACP agent on the user's own subscriptions; worktree per thread. |
| [manaflow-ai/cmux](https://github.com/manaflow-ai/cmux) | 28.8k | 01-28 / 11-13 | Ghostty-based terminal with attention rings and notifications, plus a scriptable browser pane. |
| [slopus/happy](https://github.com/slopus/happy) | 24.6k | 2025-07-18 / 11-13 | Mobile client; any ACP agent plugs in. |
| [siteboon/claudecodeui](https://github.com/siteboon/claudecodeui) | 14.6k | 2025-06-25 / 11-13 | Web and mobile UI. |
| [openclaw/acpx](https://github.com/openclaw/acpx) | 3.4k | 02-17 / 11-11 | Headless ACP client for agent-to-agent calls. |

### (l) Security
| Repo | Stars | C / P | What it does; key technique |
|---|---|---|---|
| [NVIDIA/SkillSpector](https://github.com/NVIDIA/SkillSpector) | 25.8k | 03-21 / 11-13 | Skill scanner feeding a scan, evaluate and sign pipeline. Its README reports 26.1% of 31,132 skills vulnerable and 5.2% likely malicious. |
| [trailofbits/skills](https://github.com/trailofbits/skills) | 8.2k | 01-14 / 11-12 | Audit skills. |
| [snyk/agent-scan](https://github.com/snyk/agent-scan) | 3.2k | 2025-04-07 / 11-13 | Scans local agents, MCP servers and skills for injection. |
| [cisco-ai-defense/skill-scanner](https://github.com/cisco-ai-defense/skill-scanner) | 2.7k | 01-29 / 11-10 | YARA-X plus AST dataflow, optional LLM judge, CEL decision layer. |
| [pipelab-org/pipelock](https://github.com/pipelab-org/pipelock) | 990 | 02-08 / 11-13 | Egress firewall for HTTP, MCP and A2A with signed action receipts. |
| [sondera-ai/sondera-coding-agent-hooks](https://github.com/sondera-ai/sondera-coding-agent-hooks) | 233 | 02-27 / 11-10 | Rust hook binaries with Cedar policies as a deterministic reference monitor. |

Searches for "prompt injection agent" and "agent permissions" return nothing above 15 stars.

## Hype flags
- [multica-ai/andrej-karpathy-skills](https://github.com/multica-ai/andrej-karpathy-skills): 232.8k stars for 28 commits, 20 KB, no licence, no push since April. It is a single `CLAUDE.md`.
- [VoltAgent/awesome-design-md](https://github.com/VoltAgent/awesome-design-md): 127.4k stars, 69 commits; a list.
- [cloudflare/security-audit-skill](https://github.com/cloudflare/security-audit-skill): 71.7k stars, 14 commits, 143 KB. A real but single skill.
- [tamaratran/fast-jev-compaction](https://github.com/tamaratran/fast-jev-compaction): 8.0k stars, 30 commits, pushed for one day only. The idea (delete stale tool results, never summarise) is still interesting.
- [openai/symphony](https://github.com/openai/symphony): stars far exceed the code (47 commits).
- Graphify, RTK, context-mode and ECC READMEs all funnel to paid tiers.

## Synthesis

### 1. Unmet needs, ranked by activity
1. **Behaviour shaping and reusable know-how.** Five skill repos exceed 190k stars and 42k skill repos were created in 2026. Default agents over-build, skip tests and lack domain procedure.
2. **Token cost and context bloat** (caveman, rtk, headroom, context-mode; cc-switch and [OmniRoute](https://github.com/cheaper-inference/OmniRoute) at 87.1k for provider switching). Agents read raw command output and are tied to one provider.
3. **Codebase understanding and cross-session memory** (graphify, claude-mem, codegraph, mempalace, context7). Agents re-explore by grep and forget everything each session.
4. **Running many agents at once and away from the desk** (orca, paperclip, t3code, herdr, cmux). Missing natively: worktree isolation, "who is blocked" signals, sessions that survive disconnects, phone approval.
5. **Process discipline** (spec-kit, gstack, OpenSpec, BMAD): plans that survive compaction and gated phases.
6. **Eyes on the running app** (chrome-devtools-mcp, agent-browser, playwright-mcp, agent-desktop).
7. **Visibility** into cost, context and hidden tool calls (claude-hud, ccusage, claude-devtools).
8. **Independent review** (open-code-review).
9. **Isolation and supply-chain safety.** Stars are low relative to risk, but vendor investment is serious (NVIDIA, Anthropic, Microsoft, Cisco, Snyk). This is under-served.
10. **Evaluation of harnesses and skills.** Smallest by attention, so a possible differentiator.

### 2. Ten techniques worth building in
1. Compress command output at the tool boundary with per-command filters (rtk, headroom).
2. Compact without summarising: drop stale tool results and keep text verbatim; keep the plan and task graph on disk and re-inject it each turn (planning-with-files, beads, fast-jev-compaction).
3. Progressive-disclosure skills with a built-in installer and scan-on-install (agentskills, vercel-labs/skills, SkillSpector).
4. Worktree per task, a detachable session daemon, and machine-readable working/blocked/idle state (herdr, orca, t3code).
5. A local, incremental tree-sitter code graph with caller and impact queries (graphify, codegraph).
6. A container-free OS sandbox with an egress allowlist proxy and host-side secret injection (sandbox-runtime, nono, gondolin).
7. Lifecycle hooks with deterministic policy verdicts (allow, deny, ask), for example Cedar (sondera, failproofai).
8. Browser and desktop control through accessibility-tree refs exposed as a CLI (agent-browser, agent-desktop).
9. Learning from transcripts: mine past sessions into repo-specific micro-skills and `AGENTS.md` edits, gated by evals ([microsoft/TokenEconomizer](https://github.com/microsoft/TokenEconomizer) 6.0k, [kunchenguid/backpass](https://github.com/kunchenguid/backpass) 1.5k, [tigerless-labs/autoharness](https://github.com/tigerless-labs/autoharness) 11.1k, SkillOpt).
10. A spec, plan, fresh-context subagent, independent verifier pipeline with a "proof of work" artefact (superpowers, OpenSpec, security-audit-skill, symphony).

### 3. Standards for day one
- **MCP client**, supporting both `2025-11-25` and the stateless `2026-07-28` revision, plus the MCP Registry.
- **ACP agent mode** (schema v1; track the v2 alpha). This is what makes a harness drivable by t3code, happy, Zed, acpx and others.
- **`AGENTS.md`** (read `CLAUDE.md` as a fallback) and **Agent Skills `SKILL.md`** at the paths `npx skills` installs to.
- **Claude-Code-compatible hook events and plugin/marketplace layout.** This is the de facto format; most hook tools above target it first.
- **Stable JSONL session logs and a status-line API**, so ccusage-style tools work. OpenTelemetry GenAI export is advisable but unverified here.
- **Second tier:** A2A v1.0, `DESIGN.md`, and an E2B-compatible sandbox API (several 2026 sandboxes advertise that compatibility).

No files in the project were modified; two scratch files were written to the session scratchpad only.
