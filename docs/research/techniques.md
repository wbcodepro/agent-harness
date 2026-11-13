# Engineering techniques behind effective coding agents (as of 2026-11-14)

Web search and fetch worked. openai.com returned 403 to direct fetches, so the three OpenAI index posts were read through the `r.jina.ai` reader proxy; the links below are the canonical URLs. Every linked source was fetched, and anything I could not confirm at a primary source is marked "unverified". No files were modified.

## 1. Agent loop and harness design

- **Single loop that ends when the model stops calling tools.** Codex ends a turn when the model returns an assistant message instead of a tool call ([OpenAI](https://openai.com/index/unrolling-the-codex-agent-loop/)). Anthropic defines agents as "LLMs using tools based on environmental feedback in a loop", with max-iteration stops for control ([Anthropic](https://www.anthropic.com/engineering/building-effective-agents)).
- **Minimal scaffolding, re-tested per model.** "Every component in a harness encodes an assumption about what the model can't do on its own." With Opus 4.6, Anthropic removed sprints and per-sprint evaluation but kept the planner and a final QA pass ([Anthropic](https://www.anthropic.com/engineering/harness-design-long-running-apps)). Vercel replaced about 17 specialised tools with bash over files: success went from 80% to 100% and runs were 3.5x faster with 37% fewer tokens, on only five queries ([Vercel](https://vercel.com/blog/we-removed-80-percent-of-our-agents-tools)).
- **The harness still moves scores with the model fixed.** LangChain went from 52.8% to 66.5% on Terminal-Bench 2.0 on gpt-5.2-codex by changing only the harness ([LangChain](https://www.langchain.com/blog/improving-deep-agents-with-harness-engineering)).
- **Planner/executor only where the model under-scopes.** Solo run: 20 min and $9, with a broken core feature. Planner/generator/evaluator: 6 hr and $200 ([Anthropic](https://www.anthropic.com/engineering/harness-design-long-running-apps)).
- **Steering by queue.** Amp queues messages by default and injects them at the next tool result; Esc Esc interrupts ([Amp](https://ampcode.com/news/neo)).
- **Do not prompt for upfront plans or preambles on Codex models**; it can make the model stop before finishing ([OpenAI cookbook](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide)). Dropping reasoning traces cost GPT-5-Codex 30% on Cursor Bench ([Cursor](https://cursor.com/blog/codex-model-harness)).
- **Reasoning-effort budgeting.** xhigh-only scored 53.9% (timeouts), high 63.6%, and an xhigh/high/xhigh "sandwich" 66.5% ([LangChain](https://www.langchain.com/blog/improving-deep-agents-with-harness-engineering)). Claude Code's move from high to medium default effort was reverted after quality complaints ([Anthropic](https://www.anthropic.com/engineering/april-23-postmortem)).

## 2. Context engineering

- **System prompt at the "right altitude".** Start minimal and add instructions only for observed failures ([Anthropic](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)). One line capping final responses at 100 words caused a 3% eval drop ([Anthropic](https://www.anthropic.com/engineering/april-23-postmortem)).
- **Instruction files as a map.** OpenAI keeps AGENTS.md to roughly 100 lines pointing into `docs/`; Codex caps aggregated AGENTS.md at 32 KiB ([OpenAI](https://openai.com/index/harness-engineering/), [OpenAI](https://openai.com/index/unrolling-the-codex-agent-loop/)). The evidence is mixed:
  - Vercel: an 8KB docs index in AGENTS.md scored 100%, against 53% for a skill that was never invoked in 56% of cases ([Vercel](https://vercel.com/blog/agents-md-outperforms-skills-in-our-agent-evals)).
  - ETH: context files do "not generally improve task success rates" while "increasing inference cost by over 20% on average"; repository overviews did not help ([arXiv 2602.11988](https://arxiv.org/abs/2602.11988)).
- **Just-in-time retrieval plus optional embeddings.** Claude Code is a hybrid: CLAUDE.md up front, glob/grep on demand ([Anthropic](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)). Cursor measured semantic search adding 12.5% accuracy on average (6.5% to 23.5%), and 2.6% code retention on repos of 1,000+ files; grep plus semantic was best ([Cursor](https://cursor.com/blog/semsearch)).
- **Search sub-agent.** Agents were "spending >60% of their first turn just retrieving context". SWE-grep issues up to 8 parallel calls per turn over at most 4 turns ([Cognition](https://cognition.com/blog/swe-grep)).
- **Context rot evidence.** Across 18 models, performance "grows increasingly unreliable as input length grows". Focused prompts of about 300 tokens beat full prompts of about 113k, and one distractor already hurts ([Chroma](https://www.trychroma.com/research/context-rot)).
- **Tool-result clearing.** It is the "lightest touch" compaction ([Anthropic](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)). Context editing gave 29% improvement, 39% with a memory tool, and 84% fewer tokens in a 100-turn eval ([Anthropic](https://claude.com/blog/context-management)).
- **Write large outputs to files.** Cursor saves long tool output and chat history as files the agent can `tail` or grep. MCP tool descriptions kept on disk cut tokens 46.9% in runs that used MCP ([Cursor](https://cursor.com/blog/dynamic-context-discovery)). Manus requires compression to be restorable (keep the URL or path) ([Manus](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)).
- **Compaction.**
  - Codex auto-compacts through `/responses/compact` ([OpenAI](https://openai.com/index/unrolling-the-codex-agent-loop/)).
  - Amp dropped handoff for auto-compaction at 90% full ([Amp](https://ampcode.com/news/neo)).
  - Factory's anchored incremental summary scored 3.70, against 3.44 (Anthropic) and 3.35 (OpenAI); artifact tracking was weakest for all (2.19 to 2.45). Optimise "tokens per task", not per request ([Factory](https://factory.com/news/evaluating-compression)).
  - Sonnet 4.5's "context anxiety" needed full resets ([Anthropic](https://www.anthropic.com/engineering/harness-design-long-running-apps)). Cognition's workaround was enabling the 1M beta while capping at 200k ([Cognition](https://cognition.com/blog/devin-sonnet-4-5-lessons-and-challenges)).
- **Notes and memory.**
  - Rewrite a todo file to recite goals at the end of context ([Manus](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)); rewrite `scratchpad.md` rather than append ([Cursor](https://cursor.com/blog/self-driving-codebases)).
  - Save only what cannot be re-read, keep one writer per page, and keep the injected index small: Claude Code loads the first 200 lines or 25 KB of `MEMORY.md`. Memory raised CL-Bench reward from 0.564 to 0.828 for Opus 5 ([Anthropic](https://www.anthropic.com/engineering/components-of-agent-memory)).
- **Sub-agent isolation.** Sub-agents burn tens of thousands of tokens and return "1,000-2,000 tokens" ([Anthropic](https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents)). Gemini CLI does the same and warns against parallel sub-agents for heavy edits ([Google](https://developers.googleblog.com/subagents-have-arrived-in-gemini-cli/)).

## 3. Prompt caching and cost

- **Layout: static first, dynamic last.** Claude Code orders system prompt and tools, then CLAUDE.md, then session context, then messages ([Anthropic](https://claude.dev/blog/lessons-from-building-claude-code-prompt-caching-is-everything/)). "Cache hits are only possible for exact prefix matches"; caching turns quadratic cost into linear ([OpenAI](https://openai.com/index/unrolling-the-codex-agent-loop/)).
- **What invalidates the cache.**
  - Timestamps in the system prompt, non-deterministic tool or MCP ordering, and changed tool parameters.
  - Adding or removing tools mid-session.
  - Switching model, sandbox or approval mode, or working directory.
  - On Anthropic the prefix order is tools, then system, then messages; `tool_choice`, image and thinking-parameter changes invalidate the message cache ([Anthropic docs](https://platform.claude.com/docs/en/build-with-claude/prompt-caching), plus both posts above).
- **Append, never mutate.** Send changes as a new message or a `<system-reminder>`. Implement plan mode as tools, and defer tools with stubs rather than removing them. Manus masks logits instead of removing tools ([Manus](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)).
- **Cache-safe forking.** Run compaction and side calls with the parent's identical prefix, so the fork hits the cache "at one tenth the price".
- **Monitoring.** "We run alerts on our prompt cache hit rate and declare SEVs." A bug that dropped thinking blocks caused cache misses and drained user limits ([Anthropic](https://www.anthropic.com/engineering/april-23-postmortem)).
- **Numbers.**
  - Input to output ratio is about 100:1; cached tokens cost 0.30 against 3 USD/MTok ([Manus](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus)).
  - Anthropic reads cost 0.1x base (less on the newest models); writes cost 1.25x or 2x.
  - Measured savings were 41-80% on cost and 13-31% on TTFT; naive full-context caching can increase latency ([arXiv 2601.06007](https://arxiv.org/abs/2601.06007)).

## 4. Tool design

- **Few, non-overlapping tools.** GitHub cut about 40 tools to 13 core plus embedding-routed virtual groups: 2-5 points on SWE-bench-style evals, 190 ms faster TTFT, and tool coverage of 94.5% against 69.0% static ([GitHub](https://github.blog/ai-and-ml/github-copilot/how-were-making-github-copilot-smarter-with-fewer-tools/)).
- **Naming and arguments.** Name tools after shell equivalents (`rg`) ([Cursor](https://cursor.com/blog/codex-model-harness)). Require absolute paths ([Anthropic](https://www.anthropic.com/engineering/building-effective-agents)). Take the shell command as a string with an explicit `workdir` ([OpenAI cookbook](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide)).
- **Descriptions and examples.** Input examples raised accuracy from 72% to 90% ([Anthropic](https://www.anthropic.com/engineering/advanced-tool-use)). Rewritten descriptions cut task time 40% ([Anthropic](https://www.anthropic.com/engineering/multi-agent-research-system)).
- **Errors and truncation.** Return actionable errors. Claude Code caps tool responses at 25,000 tokens ([Anthropic](https://www.anthropic.com/engineering/writing-tools-for-agents)). Codex caps at 10k tokens, split between head and tail with a truncation marker ([OpenAI cookbook](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide)).
- **Edit formats.** Across 16 models, hashline edits (line-hash anchors) averaged +15 points over `apply_patch`, with small gains over str_replace. Grok Code Fast 1 went from 6.7% to 68.3%, while GPT-5.2 Codex gained only +4.6 ([Stencil](https://stencil.so/blog/the-harness-problem); single-author benchmark). In summary:

| Format | Evidence | Source |
|---|---|---|
| `apply_patch` | OpenAI says to use the "exact `apply_patch` implementation" its models are trained on | [OpenAI cookbook](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide) |
| Apply model (full-file rewrite) | About 1000 tokens/s; diffs are "out of distribution" (2024) | [Cursor](https://cursor.com/blog/instant-apply) |
| Per-model choice | Aider selects the format per model | [Aider](https://aider.chat/docs/more/edit-formats.html) |

- **Code mode / programmatic tool calling.**
  - 150,000 down to 2,000 tokens ([Anthropic](https://www.anthropic.com/engineering/code-execution-with-mcp)).
  - 37% fewer tokens ([Anthropic](https://www.anthropic.com/engineering/advanced-tool-use)).
  - 2,500+ endpoints behind `search()` and `execute()` in about 1,000 tokens, against 1.17M ([Cloudflare](https://blog.cloudflare.com/code-mode-mcp/)).
- **Tool search.** Deferred loading cut tool-definition context from about 77K to 8.7K tokens and raised Opus 4.5 accuracy from 79.5% to 88.1% ([Anthropic](https://www.anthropic.com/engineering/advanced-tool-use)).
- **Parallel calls.** "Never read files one-by-one unless logically unavoidable" ([OpenAI cookbook](https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide)). Going from 4 to 8 parallel calls cut search turns from 6 to 4 ([Cognition](https://cognition.com/blog/swe-grep)).

## 5. Verification and reliability

- **Pre-completion verification hook.** The top failure was that the agent "re-read its own code, confirmed it looks ok, and stopped". A hook forces a check against the spec before exit; loop detection nudges after N edits to one file ([LangChain](https://www.langchain.com/blog/improving-deep-agents-with-harness-engineering)).
- **Lint and LSP feedback.** Custom linter errors carry remediation text ([OpenAI](https://openai.com/index/harness-engineering/)). Codex only called `read_lints` when explicitly told to ([Cursor](https://cursor.com/blog/codex-model-harness)). OpenCode feeds LSP diagnostics to the agent ([OpenCode](https://opencode.ai/docs/lsp/)). Measured LSP gains: unverified.
- **Agent-friendly test output.** Keep the console terse, send logs to files, put `ERROR` on one line, and offer a `--fast` 1%/10% sample; models are time-blind ([Anthropic](https://www.anthropic.com/engineering/building-c-compiler)).
- **Explicit done criteria.** Keep a JSON feature list where the agent may only flip `passes`, and test end to end as a user ([Anthropic](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)).
- **Separate, sceptical evaluator.** Tuning an evaluator is "far more tractable than making a generator critical of its own work". Devin Review, with clean context, catches an average of 2 bugs per PR, about 58% of them severe ([Cognition](https://cognition.com/blog/multi-agents-working)).
- **Checkpoints.** Claude Code snapshots per prompt; bash-only changes and most sub-agent edits are not tracked ([Claude Code docs](https://code.claude.com/docs/en/checkpointing)). Amp removed rollback as "always best-effort" ([Amp](https://ampcode.com/news/neo)).
- **Flaky environments.** Rerun flaky tests rather than block ([OpenAI](https://openai.com/index/harness-engineering/)). Disposable sandboxes surface failures as tool errors ([Anthropic](https://www.anthropic.com/engineering/managed-agents)).

## 6. Long-running and multi-agent work

- **Hand-offs through artifacts.** Use an initializer session, `init.sh`, a progress file, git commits, and one feature per session ([Anthropic](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)). Keep an append-only session log outside the context so a crashed harness can resume ([Anthropic](https://www.anthropic.com/engineering/managed-agents)).
- **Hierarchy beats a flat swarm.** With shared locks, 20 agents fell to the throughput of 1-3. Recursive planners plus isolated workers with their own repo copies peaked at about 1,000 commits per hour. The integrator role was removed, and a "small and constant" error rate is tolerated ([Cursor](https://cursor.com/blog/self-driving-codebases), [Cursor](https://cursor.com/blog/scaling-agents)).
- **Git as coordinator.** 16 agents used lock files, cost just under $20,000, and produced a 100k-line compiler. An oracle (GCC) was needed to make one giant task parallelisable ([Anthropic](https://www.anthropic.com/engineering/building-c-compiler)).
- **Worktree-per-change** with its own app instance and observability stack ([OpenAI](https://openai.com/index/harness-engineering/)).
- **When it helps or hurts.**
  - +80.9% on parallelisable tasks, 39-70% worse on sequential ones; errors amplified 17.2x (independent agents) against 4.4x (centralised) ([Google](https://research.google/blog/towards-a-science-of-scaling-agent-systems-when-and-why-agent-systems-work/)).
  - 90.2% better on research at about 15x chat tokens; coding has fewer parallelisable tasks ([Anthropic](https://www.anthropic.com/engineering/multi-agent-research-system)).
  - Keep writes single-threaded; unstructured swarms are "mostly a distraction" ([Cognition](https://cognition.com/blog/multi-agents-working)).

## 7. Safety

- **Approval fatigue.** Users accept 93% of prompts ([Anthropic](https://www.anthropic.com/engineering/claude-code-auto-mode)). An OS sandbox (Seatbelt, bubblewrap) with a network proxy cut prompts 84% ([Anthropic](https://www.anthropic.com/engineering/claude-code-sandboxing)). Codex uses the same OS primitives, with `workspace-write` as the local default ([OpenAI docs](https://learn.chatgpt.com/docs/sandboxing)).
- **Classifier auto-approval.**
  - Claude Code uses a two-stage classifier that sees only user messages and tool calls. False-positive rate is 0.4%; it misses 17% of real overeager actions. It escalates after 3 consecutive or 20 total denials ([Anthropic](https://www.anthropic.com/engineering/claude-code-auto-mode)).
  - Codex Auto-review stops "roughly 200x less often". 7.2% of actions leave the sandbox and 99.1% of those are approved; recall is 90.3% on overeagerness ([OpenAI](https://alignment.openai.com/auto-review)).
- **Default-allow.** Amp removed prompts entirely: "near-impossible to determine statically whether a tool invocation will be destructive" ([Amp](https://ampcode.com/news/neo)).
- **Model-level injection defences are not sufficient.**
  - 12 defences were bypassed "with attack success rate above 90% for most" ([arXiv 2510.09023](https://arxiv.org/abs/2510.09023)).
  - Red-team exfiltration of `~/.aws/credentials` succeeded 24 of 25 times; only egress and filesystem boundaries stop it. Allowlists are capability grants ([Anthropic](https://www.anthropic.com/engineering/how-we-contain-claude)).
  - Rule of Two: at most two of untrusted input, sensitive access, and external action ([Meta](https://ai.meta.com/blog/practical-ai-agent-security/)).
- **Secrets stay outside the sandbox.** Use a git proxy with scoped credentials and a vault-backed MCP proxy ([Anthropic](https://www.anthropic.com/engineering/managed-agents)). Codex stores credentials in the OS keyring and exports OpenTelemetry audit logs ([OpenAI](https://openai.com/index/running-codex-safely/)).
- **Trade-offs.** A VM is stronger but boots slower and hides activity from endpoint tooling; run the loop on the host and only execution in the VM ([Anthropic](https://www.anthropic.com/engineering/how-we-contain-claude)).

## 8. Evaluation

- **SWE-bench Verified is discredited at the frontier.** 59.4% of 138 audited hard problems were flawed, and models reproduce gold patches ([OpenAI](https://openai.com/index/why-we-no-longer-evaluate-swe-bench-verified/)). Roughly half of test-passing PRs would not be merged, a gap of about 24.2 points ([METR](https://metr.org/notes/2026-03-10-many-swe-bench-passing-prs-would-not-be-merged-into-main/)).
- **SWE-bench Pro** has 1,865 problems across 41 repos with held-out and commercial sets ([arXiv 2509.16941](https://arxiv.org/abs/2509.16941)). The public top score is 61.50 ± 3.10, with harness and turn limits varying by entry ([Scale](https://labs.scale.com/leaderboard/swe_bench_pro_public)). A reported grader-error audit of Pro: unverified.
- **Terminal-Bench 2.0** has 89 tasks; frontier agents resolved under 65% at publication ([arXiv 2601.11868](https://arxiv.org/abs/2601.11868)). Current leaderboard figures: unverified (the page would not render). Resource limits alone move scores 6 points, so gaps under 3 points "deserve skepticism" ([Anthropic](https://www.anthropic.com/engineering/infrastructure-noise)).
- **Own evals.**
  - Start with 20-50 tasks from real failures, grade outcomes rather than paths, isolate trials, track pass^k, and read transcripts ([Anthropic](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents)).
  - Run parallel trace-analysis agents over failures ([LangChain](https://www.langchain.com/blog/improving-deep-agents-with-harness-engineering)).
  - Run per-model evals and ablations for every system-prompt change ([Anthropic](https://www.anthropic.com/engineering/april-23-postmortem)).
- **Metrics.** Accuracy, runtime, tool-call count, tokens and tool errors ([Anthropic](https://www.anthropic.com/engineering/writing-tools-for-agents)); tokens per task ([Factory](https://factory.com/news/evaluating-compression)); cache hit rate; approval-stop rate.

## The 15 highest-leverage techniques for a new harness

1. **Cache-stable, append-only prompt.** It dominates cost and latency at 100:1 input to output ([Anthropic](https://claude.dev/blog/lessons-from-building-claude-code-prompt-caching-is-everything/)).
2. **One simple loop, minimal scaffolding, ablate per model.** Scaffolding goes stale as models improve ([Anthropic](https://www.anthropic.com/engineering/harness-design-long-running-apps)).
3. **OS sandbox with deny-by-default network and secrets outside.** It is the only defence that held in red-teaming, and it cuts prompts 84% ([Anthropic](https://www.anthropic.com/engineering/how-we-contain-claude)).
4. **Pre-completion verification against the spec.** It fixes the commonest failure ([LangChain](https://www.langchain.com/blog/improving-deep-agents-with-harness-engineering)).
5. **Small core toolset built around shell and files.** Fewer choices means higher success ([Vercel](https://vercel.com/blog/we-removed-80-percent-of-our-agents-tools)).
6. **Per-model edit tool with stale-read rejection.** Format alone moves success by double digits on some models ([Stencil](https://stencil.so/blog/the-harness-problem)).
7. **Bounded tool output with spill-to-file.** It prevents context rot without losing data ([Cursor](https://cursor.com/blog/dynamic-context-discovery)).
8. **Tool-result clearing, then cache-safe compaction.** It measured 84% fewer tokens ([Anthropic](https://claude.com/blog/context-management)).
9. **Read-only search and review sub-agents with clean context.** They isolate noise and catch bugs the author misses ([Cognition](https://cognition.com/blog/multi-agents-working)).
10. **Deferred tool loading and code mode for MCP.** 85-99.9% fewer definition tokens ([Cloudflare](https://blog.cloudflare.com/code-mode-mcp/)).
11. **Classifier auto-approval over sandbox escapes only.** It replaces rubber-stamping, with published error rates ([Anthropic](https://www.anthropic.com/engineering/claude-code-auto-mode)).
12. **Durable external state: progress file, feature list, git, session log.** It lets work survive context resets and crashes ([Anthropic](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)).
13. **Short, map-style instruction file plus mechanical lint enforcement.** Passive context wins where long files cost 20% more ([OpenAI](https://openai.com/index/harness-engineering/)).
14. **Parallel read-only tool calls and queued steering.** Fewer turns and fewer interruptions ([Cognition](https://cognition.com/blog/swe-grep)).
15. **In-house eval suite with noise controls and transcript review.** Public leaderboards are contaminated and noisy ([Anthropic](https://www.anthropic.com/engineering/demystifying-evals-for-ai-agents)).

## Open problems nobody has solved well

1. **Artifact and file-state tracking through compaction.** Every method scored 2.19-2.45 out of 5 ([Factory](https://factory.com/news/evaluating-compression)).
2. **Prompt injection.** Adaptive attacks beat published defences, and classifiers still miss 10-17% of overeager actions.
3. **Parallel writers.** Systems rely on single-threaded writes or tolerated error rates; manager-to-child communication remains weak ([Cognition](https://cognition.com/blog/multi-agents-working)).
4. **Mergeability versus test passing.** No benchmark measures maintainer-grade quality at scale ([METR](https://metr.org/notes/2026-03-10-many-swe-bench-passing-prs-would-not-be-merged-into-main/)).
5. **Benchmark validity.** Contamination, grader flaws, infrastructure noise and harness-dependent scores all persist.
6. **Reliable rollback.** Shell side effects and sub-agent edits escape checkpoints.
7. **Instruction and memory files.** Evidence conflicts between Vercel and ETH, and memory bloat or staleness needs offline consolidation ([Anthropic](https://www.anthropic.com/engineering/components-of-agent-memory)).
8. **Drift and time-blindness in long runs.** Periodic fresh starts and time-budget nudges remain workarounds ([Cursor](https://cursor.com/blog/scaling-agents)).
