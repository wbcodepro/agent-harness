# Closing the capability gap for weak models in a coding harness: evidence review (2026-11-14)

**Method caveats.** Web search and fetch worked. Pages were read through a summarising fetcher, so table transcriptions can carry small errors. `tbench.ai` and `swebench.com` leaderboards rendered empty; Terminal-Bench numbers come from the TB2 paper, the Meta-Harness paper and a June 2026 survey. Anything not confirmed in a primary source is marked **unverified**.

## 1. Does the harness matter more for weak models?

It matters for every model, and the relative effect is largest for weak ones. Model choice still dominates in absolute terms.

**Same model, different harness (Terminal-Bench 2.0)**

| Model | Lowest | Highest | Source |
|---|---|---|---|
| Claude Haiku 4.5 | 13.9% (OpenHands) | 37.6% (Meta-Harness) | [arXiv 2603.28052](https://arxiv.org/html/2603.28052v1) |
| Claude Opus 4.6 | 58.0% (Claude Code) | 81.8% (ForgeCode) | same |
| GPT-5 | 33.9% (Mini-SWE-Agent) | 49.6% (Codex CLI) | [survey arXiv 2606.20683 §7.4](https://arxiv.org/pdf/2606.20683) |
| Grok Code Fast 1 | 14.5% (Terminus 2) | 24.5% (Mini-SWE-Agent) | [TB2 paper arXiv 2601.11868](https://arxiv.org/html/2601.11868) |

- Haiku 4.5 also scored 27.5% (Claude Code), 28.3% (Terminus 2), 29.8% (Mini-SWE-Agent), 33.7% (Terminus-KIRA) and 35.5% (Goose). Haiku's spread is 2.7x; Opus's is 1.4x.
- The survey finds a median within-model range of 13.6 points across 20 models with at least three harness results. The largest spreads (over 20 points) include Haiku 4.5.
- Meta-Harness's main addition (a pre-loop environment snapshot) gave +3.9 points on Haiku against +1.7 on Opus, relative to Terminus-KIRA.
- LangChain moved deepagents-cli from 52.8% to 66.5% on TB2 with gpt-5.2-codex fixed. The changes were a verify loop, a pre-completion checklist, startup context injection, loop detection and time-budget warnings ([LangChain](https://www.langchain.com/blog/improving-deep-agents-with-harness-engineering)).

**Same harness, different model**

- The TB2 paper concludes model choice outweighs scaffold: under Codex CLI, GPT-5-Nano scores 11.5%, GPT-5-Mini 31.9% and GPT-5.2 62.9%.
- Under Terminus 2, GPT-OSS-20B scores 3.1% and GPT-OSS-120B 18.7%.
- A Qwen 3 Coder trajectory analysis shows a "more balanced error profile, with higher rates across all failure modes", whereas frontier models fail mostly on execution. Weak models therefore need coherence and verification help as well as better tools.

## 2. Edit mechanics

- **Hash-anchored lines.** "The Harness Problem" tested 16 models on 3 edit tools, with 3 runs of 180 tasks each ([Bölük](https://stencil.so/blog/the-harness-problem)).
  - Grok Code Fast 1 went from 6.7% (patch) to 68.3% (hashline).
  - Patch-format failure rates were 50.7% for Grok 4 and 46.2% for GLM-4.7.
  - Grok 4 Fast used 61% fewer output tokens.
  - Per-model deltas against plain replace were small and sometimes negative (my reading of a garbled table, so **unverified**).
- **Counter-evidence.** A 20-task-per-language replication found replace beat hashline in 6 of 9 cells; gpt-4.1-mini on Python scored 65% with replace and 50% with hashline. Its conclusion is that the model gap exceeds the format gap. The author calls it not statistically rigorous ([nwyin](https://dev.to/nwyin/hashline-vs-replace-does-the-edit-format-matter-15n2)).
- **Search/replace against unified diff.** In Diff-XYZ, search-replace wins for small open models: Qwen2.5-Coder-7B generates correct diffs at 0.28 EM against 0.06 for udiff, and the 32B at 0.68 against 0.23. GPT-4.1-nano is the reverse, at 0.07 against 0.50 ([arXiv 2510.12487](https://arxiv.org/html/2510.12487)). The best format is model-specific and has to be measured.
- **Whole-file.** On Aider polyglot, Qwen2.5-Coder-32B scored 16.4% with `whole` (99.6% well-formed) and 8.0% with `diff` (71.6% well-formed) ([Aider leaderboard](https://aider.chat/docs/leaderboards/)). gpt-oss-120b was only 79.1% well-formed on diff.
- **Lenient application.** Disabling Aider's flexible patching caused a "9X increase in editing errors". Unified diffs raised GPT-4 Turbo from 20% to 61% on its laziness benchmark ([Aider](https://aider.chat/docs/unified-diffs.html)).
- **Per-model defaults in practice.** Aider's `model-settings.yml` has 357 model entries: 289 use `diff`, 35 `diff-fenced`, 4 `udiff`, 4 `whole`, and 138 define a separate `editor_edit_format` (repo `Aider-AI/aider`).

## 3. Tool-calling reliability

- **Constrained decoding fixes syntax but can damage semantics.** On sub-3B models, schema validity rose from 61.5% to 100% while answer accuracy fell from 19.7% to 11.0%. Qwen2.5-1.5B dropped from 91.5% to 48.0% executable accuracy under a hard tool schema. The recommendation is "reason free, constrain late" ([arXiv 2605.26128](https://arxiv.org/abs/2605.26128)).
- **Parser and serving-stack bugs can dominate.** In a Gemma 4 31B study, vLLM's parser dropped 34 of 60 `edit_file` calls at inner quotes. Three prompt rules cut that to 1 and timeouts from 7 to 1 (repo `shawnbaako/gemma4-agent-failure-analysis`).
- **Serving stacks differ.** Ollama gates `tools=` per model by a template flag and rejects some models before inference. Adding a text tool list alongside the native channel recovers fidelity, but a uniform text protocol hurts models with native support. Constrained decoding "can cause non-termination" ([REALM 2026](https://aclanthology.org/2026.realm-1.7/)).
- **Tool naming.** Renaming tools to names the model expects improved small-model accuracy by up to 17% and cut invented-tool-name errors by 80% ([arXiv 2510.07248](https://arxiv.org/abs/2510.07248)).
- **Examples in tool definitions.** Accuracy on complex parameters rose from 72% to 90% in Anthropic's internal tests ([Anthropic](https://www.anthropic.com/engineering/advanced-tool-use)).
- **Bash-only text protocol.** mini-swe-agent has no tools other than bash and "doesn't even need to use the tool-calling interface", so it runs with any model (repo `SWE-agent/mini-swe-agent`).

## 4. Decomposition and fixed pipelines

- **Agentless** (localise, repair, validate; GPT-4o; SWE-bench Lite): 32.00% at $0.70 per issue, against SWE-agent with GPT-4o at 18.33% and $2.53 ([arXiv 2407.01489v2](https://arxiv.org/html/2407.01489v2)). This is the clearest case of a fixed pipeline lifting an older model, at about 28% of the cost.
- **Architect/editor** (Aider code-editing benchmark, [Aider](https://aider.chat/2024/09/26/architect.html)):

| Architect | Editor | Solo architect | Paired |
|---|---|---|---|
| gpt-4o-mini | gpt-4o-mini (whole) | 55.6% | 60.2% |
| o1-mini | deepseek (whole) | 61.1% | 71.4% |
| o1-preview | deepseek (whole) | 79.7% | 85.0% |

  The gain is largest for models that reason adequately but format badly. The local pair QwQ-32B + Qwen2.5-Coder reached only 26.2% on polyglot, so the split does not rescue weak reasoning.
- **Cost of heavy orchestration.** Anthropic's planner/generator/evaluator harness cost $200 over 6 hours against $9 over 20 minutes solo ([Anthropic](https://www.anthropic.com/engineering/harness-design-long-running-apps)).

## 5. Verification

- **Candidate selection (Agentless ablation).** Majority voting alone gave 25.67%, adding regression tests 27.00%, adding generated reproduction tests 32.00%. The last step cost about $0.25 per issue. One greedy sample solved 80 issues, 40 samples with selection solved 96, and an oracle selector would have solved 126.
- **Sampling diversity for a small model.** Gemma 4 31B solved 16–18 of 42 per sample at temperature 0.7, and 21 of 42 in union. "Sampling diversity is worth more than time": the last two minutes of a six-minute budget added 7% of solves.
- **CodeMonkeys.** 57.4% on SWE-bench Verified with 10 candidates and test voting; oracle selection would give 69.8% and random about 45.8% (search summary of [arXiv 2501.14723](https://arxiv.org/abs/2501.14723); paper not opened).
- **A negative result.** A forced "reproduce the bug first" rule was followed on 20 of 23 terse Gemma tasks and solved 3 of 23, the same as without it. Appending the missing issue text raised solves from 11% to 44%. Verification cannot substitute for missing specification.

## 6. Context help

- **Chroma "Context Rot"** (18 models): performance "consistently degrades with increasing input length", and a single distractor already hurts. It gives no clean size-tier comparison, so "small models rot faster" is **unverified** ([Chroma](https://trychroma.com/research/context-rot)).
- **Skeletons beat full files for localisation.** Agentless: 58.33% at $0.02 against 53.67% at $0.15. Combining prompt and embedding retrieval reached 81.67% file-level recall for $0.06.
- **Procedural knowledge (SkillsBench).** Curated skills raised average pass rate from 33.9% to 50.5% across 18 configurations; "smaller models with Skills can match larger models without them" ([arXiv 2602.12670](https://arxiv.org/abs/2602.12670)). Haiku 4.5 with skills at 27.7% against Opus 4.5 without at 22.0% comes from secondary coverage, so **unverified**.
- **Per-model catalogs exist in production.** Codex's `ModelInfo` carries `shell_type`, `apply_patch_tool_type`, `truncation_policy`, `context_window`, `auto_compact_token_limit`, `effective_context_window_percent`, reasoning levels and per-model instruction messages (`openai/codex`, `codex-rs/protocol/src/openai_models.rs`).

## 7. Cascades and advisors

- **Anthropic advisor tool.** Sonnet 4.6 with an Opus 4.6 advisor gained 2.7 points on SWE-bench Multilingual at 11.9% lower cost. Haiku 4.5 with advisor scored 41.2% on BrowseComp against 19.7% solo, which trails Sonnet solo by 29% at 85% lower cost. Advisor output is 400–700 tokens. All vendor-measured ([Anthropic](https://claude.com/blog/the-advisor-strategy)).
- **SWE-Protégé.** A fine-tuned Qwen2.5-Coder-7B consulting Opus 4.1 about four times per task (roughly 11% of tokens) reached 42.4% on SWE-bench Verified, against 17% for the prior 7B baseline. Untrained consult policies did much worse: fixed-interval calls 19.6%, random 24.2%. Before RL, 31% of trajectories looped for more than 10 steps ([arXiv 2602.22124](https://arxiv.org/html/2602.22124v1)). The harness, not the weak model, should decide when to escalate.
- **Escalate on test failure.** A DeepSeek V4 Pro to GPT-5.6 Sol cascade on DeepSWE reached 83.0% at $3.35 per task, against 81.0% for GPT-5.6 Sol with two attempts. It was reconstructed from pass@4 logs with held-out tests as an oracle, not run live. DeepSeek alone with four attempts reached 88.5% ([LDS](https://letsdatascience.com/news/together-ai-tells-lds-what-its-3-35-coding-result-leaves-out-3dab320a)).
- **Routing papers.** SWE-Router ([arXiv 2607.00053](https://arxiv.org/abs/2607.00053)) routes on a cheap model's partial trajectory; the abstract has no numbers. Triage ([arXiv 2604.07494](https://arxiv.org/html/2604.07494)) is a proposal with no results.

## 8. Local and open-weight specifics

- **Credible models (Vals SWE-bench Verified, mini-SWE-agent, 2026-09-01).** DeepSeek V4 Pro 96.4% at $0.10 per task, GLM 5.3 95.4%, Kimi K3 93.4%, DeepSeek V4 Flash 88.8% at $0.01, Qwen 3.8 27B 86.0%, Devstral 2 62.8%, GPT OSS 120B 33.6%. Claude Opus 5 leads at 97.0% ([Vals](https://www.vals.ai/benchmarks/swebench)). Open-weight status of each is my assumption. The benchmark is saturated and probably contaminated.
- **Context.** Ollama defaults to 4k below 24 GiB VRAM, 32k for 24–48 GiB, 256k above. It says agents and coding tools "should be set to at least 64000 tokens" ([Ollama](https://docs.ollama.com/context-length)).
- **Sampling.** Qwen3-Coder-30B recommends temperature 0.7, top_p 0.8, top_k 20, repetition_penalty 1.05, with no thinking mode ([HF](https://huggingface.co/Qwen/Qwen3-Coder-30B-A3B-Instruct)). The Gemma study used 0.2.
- **llama.cpp.** It needs `--jinja`; unrecognised templates fall to a generic handler that "may consume more tokens". Parallel tool calls are off by default. Extreme KV-cache quantisation "can substantially degrade the model's tool calling performance" ([docs](https://github.com/ggml-org/llama.cpp/blob/master/docs/function-calling.md)).
- **Quantisation.** 4-bit AWQ/GPTQ gave no statistically significant accuracy loss on Qwen2.5-Coder 7–32B and Devstral-Small, but behaviour shifted: Qwen-7B one-shot solves fell from 17 to 0 while loop solves rose from 10 to 25 ([NeurIPS 2026](https://neurips.cc/virtual/2026/171237)). Repair loops matter more under quantisation.
- **Fine-tuning pitfall.** A wrong loss mask on Gemma 4's `<|tool_response>` stop token caused up to 488 tool calls in one turn. A harness needs a calls-per-turn cap.

## 9. Limits

- No scaffold found lifts nano-class models to frontier level on hard terminal tasks: GPT-5-Nano's best is 11.5% and GPT-OSS-20B's 3.4%.
- Heavy scaffolds do not help strong models on saturated tasks. On Vals, Opus 4.8 scores 88.6% with mini-SWE-agent and 85.8% with Claude Code; GPT 5.5 scores 82.6% with mini-SWE-agent, 76.4% with Codex and 76.2% with Factory.
- Anthropic removed context resets at Opus 4.5 and sprint decomposition at Opus 4.6. An evaluator "is worth the cost when the task sits beyond what the current model does reliably solo".
- More reasoning can hurt: xhigh throughout scored 53.9% against 63.6% at high, because of timeouts (LangChain).
- Vercel's tool removal (18 tools to one bash tool, 4/5 to 5/5 on five queries, about 37% fewer tokens, Opus 4.5) is **unverified** from search summary and tiny in sample.

## (a) Model capability profile

| Field | Harness behaviour it drives |
|---|---|
| `tool_protocol`: native, native plus text list, text/XML, bash-only | Parser choice; whether tools are also listed in the prompt |
| `constrain`: none, late-JSON, grammar | Constrain only the final call, never the reasoning |
| `arg_repair`, `max_calls_per_turn`, `parallel_tools` | Quote/JSON repair, runaway-turn cap, serial execution |
| `edit_format`: whole (with size cap), search-replace, udiff, hashline, apply-patch; `fuzzy_apply` level | Which edit tool is exposed; fallback chain on failure |
| `edit_wellformed_rate` (measured by probe) | Auto-downgrade the edit format below a threshold |
| `ctx_window`, `effective_ctx`, `compact_at` | Budgets, compaction or reset; refuse to start below a minimum |
| `tool_set`: minimal or full; `tool_examples` | Number of tools; few-shot examples in descriptions |
| `prompt_template`, `reminders` | Per-family system prompt; restating rules late in context |
| `sampling`: temperature, top_p, top_k, repetition penalty; `reasoning_effort` per phase | Vendor defaults; phase-specific effort |
| `workflow`: free loop, plan-then-act, fixed pipeline | How many decisions are removed from the model |
| `verify`: off, gates, checklist, n-candidate | Compile/test gates, pre-completion check, candidate count |
| `escalation`: advisor model, triggers (loop, repeated edit failure, test failure), `max_uses` | Harness-driven consults and hand-offs |
| `serving_quirks`: chat template, KV-quantisation warning, stop tokens | Start-up checks and warnings |

Populate the profile with a short probe suite per model and serving stack. Evidence suggests formats do not transfer across models.

## (b) Twelve techniques ranked by accuracy per extra token

1. **Fix tool-call parsing and argument repair**: near-zero tokens; 34 dropped edits to 1 (Gemma study).
2. **Per-model edit format with lenient apply**: 8.0% to 16.4% for Qwen2.5-Coder-32B; 9x fewer edit errors (Aider); 6.7% to 68.3% (Bölük).
3. **Supply the missing specification** (issue text, startup environment snapshot): 11% to 44% (Gemma study); +3.9 points on Haiku (Meta-Harness).
4. **Right-size context** (64k minimum, skeleton views): the cheaper skeleton is also more accurate (Agentless).
5. **Pre-completion checklist and test gate, plus loop detection**: part of 52.8% to 66.5% (LangChain; not ablated separately).
6. **Small tool set, familiar names, examples**: up to +17% (PA-Tool); 72% to 90% (Anthropic).
7. **Fixed localise, repair, validate pipeline for weak models**: 18.33% to 32.00% at 28% of the cost (Agentless against SWE-agent, GPT-4o).
8. **Curated skills**: +16.6 points on average (SkillsBench).
9. **Architect/editor split**: +4.6 to +10.3 points (Aider).
10. **Harness-triggered advisor**: +2.7 points at −11.9% cost (Anthropic); 17% to 42.4% for a 7B model (SWE-Protégé, needs training).
11. **Multiple candidates with test-based selection**: 80 to 96 solves for 40x samples (Agentless). Effective but token-heavy.
12. **Late-only constrained decoding**: removes parse failures, but hard schemas cost accuracy (Constraint Tax).

## (c) How close weak models get

- **Routine repository bug-fixing.** The gap is now closed mainly by newer cheap models, not by scaffolds: DeepSeek V4 Flash reaches 88.8% at $0.01 per task with a bash-only loop, against 97.0% for the leader.
- **Mid-tier models on hard terminal tasks.** Harness work is worth roughly 10–24 points. Haiku 4.5's best harness (37.6%) is still about half of Opus 4.6 on the same harness (76.4%).
- **Small local models (7–30B).** Best evidence is 42.4% on SWE-bench Verified for a fine-tuned 7B with a frontier advisor, against 72% cited for Sonnet 4.5. That is roughly 60% of frontier, and it needed training and paid consults.
- **Nano or 20B-class models on hard tasks.** They stay at 3–12% under every harness tested.

Overall, a harness can reliably remove mechanical losses (edit, tool-call, context and termination failures), which make up most of a weak model's deficit on easy and medium tasks. On hard tasks the evidence shows it closing perhaps a quarter to a half of the gap, with the rest needing a stronger model or escalation to one.
