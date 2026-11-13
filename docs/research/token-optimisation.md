# Token optimisation for coding agents: research report (2026-11-14)

Web search, page fetch and `gh` all worked. Numbers are quoted as the sources give them; "vendor" means self-measured. Three sources could not be read and are marked unverified: both JetBrains blog posts and OpenAI's "Unrolling the Codex agent loop" (all HTTP 403), and Anthropic's old token-efficient-tool-use page (now redirects to the migration guides). Nothing on this machine was modified apart from README copies in the session scratchpad.

## 1. Provider prompt caching

All providers match on an exact prefix. One changed byte invalidates everything after it.

**Anthropic** ([caching docs](https://platform.claude.com/docs/en/build-with-claude/prompt-caching))
- **Breakpoints:** explicit `cache_control`, max 4, or one top-level automatic breakpoint. Each breakpoint looks back at most 20 positions.
- **Render order:** tools, then system, then messages.
- **Price multipliers:** write 1.25x (5 min) or 2x (1 h). Read 0.1x; 0.05x on Opus 5.5 and Sonnet 5.5; 0.025x on Fable/Mythos 5.1 and 5.2.
- **Minimum prefix:** 512 tokens on the newest models, 1,024 on Opus 4.8, Sonnet 5 and Sonnet 4.x, 2,048 on Opus 4.7, 4,096 on Opus 4.6/4.5 and Haiku 4.5. Shorter prefixes silently do not cache.
- **TTL:** counted from request start, refreshed free on every read.
- **Isolation:** per workspace (per organisation on Bedrock and Google-managed), and per model.
- **What breaks it:** tool definition changes (everything); web-search, citations or speed toggles (system and messages); `tool_choice` or images (messages); thinking or effort changes (messages, model-specific).
- **Measure:** `cache_read_input_tokens` and `cache_creation_input_tokens`.

| Model ($/MTok) | Input | 5m write | 1h write | Read | Output |
|---|---|---|---|---|---|
| Fable 5.2 | 8 | 10 | 16 | 0.20 | 40 |
| Opus 5.5 | 4 | 5 | 8 | 0.20 | 20 |
| Sonnet 5.5 | 2 | 2.50 | 4 | 0.10 | 10 |
| Haiku 5.5 (≤100K) | 0.10 | 0.125 | 0.20 | 0.01 | 0.50 |

**OpenAI** ([caching guide](https://developers.openai.com/api/docs/guides/prompt-caching), [pricing](https://developers.openai.com/api/docs/pricing))
- **Mode:** automatic. GPT-5.6 and later add explicit breakpoints, `prompt_cache_options.ttl` of `30m` (default) or `12h`, and a `prewarm` call.
- **Price multipliers (5.6+):** write 1.25x or 2.5x; read 0.1x (0.05x on gpt-6.1-sol and gpt-6.1-terra). Earlier models have no write fee.
- **Minimum:** 1,024 tokens.
- **Routing:** by a hash of the initial tokens plus `prompt_cache_key`. "Traffic above 15 requests per minute can lead to overflow routing."
- **Advice:** the guide says to avoid compaction or truncation that changes the prefix, and to use `tool_choice: "none"` or `allowed_tools` instead of removing tools.
- **Measure:** `usage.input_tokens_details.cached_tokens` and `cache_write_tokens`.
- **Prices (input / cached / output):** gpt-6.1-sol $2 / $0.10 / $10; gpt-5.6-sol $4 / $0.40 / $20; gpt-5.3-codex $1.75 / $0.175 / $14.

**DeepSeek** ([cache guide](https://api-docs.deepseek.com/guides/kv_cache), [pricing](https://api-docs.deepseek.com/quick_start/pricing))
- **Mode:** automatic disk cache, best-effort, with no write fee. The docs state no minimum and no fixed TTL ("a few hours to a few days").
- **Measure:** `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`.
- **Prices (hit / miss / output, peak):** deepseek-flash $0.006 / $0.30 / $1.20; deepseek-v4-pro $0.044 / $1.32 / $3.96. Off-peak is half.
- **Consequence:** a hit costs 2–3% of a miss, so prefix stability matters more here than on any other provider.

**Google** ([caching](https://aistudio.google.com/docs/caching), [pricing](https://aistudio.google.com/docs/pricing))
- **Mode:** implicit caching on 2.5 and later. Explicit cache objects are not supported on Gemini 4.
- **Minimum:** 4,096 tokens (Gemini 4, 3.x); 2,048 (2.5).
- **Gemini 4 quirk:** agent loops are cached in full only if the new request adds at least 2,050 tokens.
- **Prices:** Gemini 4 Carbon $2 in / $0.10 cached / $10 out until 2026-11-30, doubling afterwards. Explicit cache storage is $0.50–$4.50 per MTok per hour on older models.
- **Measure:** `usage.total_cached_tokens`.

**Others** ([OpenRouter table](https://openrouter.ai/docs/features/prompt-caching)): xAI and Moonshot automatic, read 0.25x; Groq 0.5x; Qwen explicit, 5 min, 1.25x write, 0.1x read. This table disagrees with the OpenAI and Google primary pages, so treat it as stale.

**Cache-aware layout**

Sources: [Claude Code caching docs](https://code.claude.com/docs/en/prompt-caching.md), [Anthropic's "prompt caching is everything" post](https://claude.dev/blog/lessons-from-building-claude-code-prompt-caching-is-everything/), [Manus](https://manus.im/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus).

- Order content static to dynamic.
- Keep timestamps out of the system prompt.
- Serialise tools deterministically.
- Treat history as append-only.
- Deliver state changes as messages (`<system-reminder>`), not as system-prompt edits.
- Model modes as tools; do not swap the tool set.
- Run compaction as a fork that reuses the parent's exact prefix.
- Claude Code alerts on cache hit rate and declares SEVs when it drops.
- Manus reports a 100:1 input-to-output ratio and calls KV-cache hit rate the most important metric.

**Measured (Anthropic, internal, list prices)** ([cost guide](https://platform.claude.com/docs/en/about-claude/models/optimizing-for-cost-and-intelligence.md))
- **Without caching:** cost was 3.3x (SWE-bench Pro, Opus 5.5), 3.1x (Sonnet 5.5), 1.7x (short Haiku sessions) and 7.3x (bug-triage agent).
- **Healthy loop:** reads a median 84% of input from cache; below about 80%, look for a breaker.
- **System-prompt edit at about 100K context:** costs 25–50x the same text sent as a mid-conversation message.
- **TTL choice:** 5 min beats 1 h by 15–19% when turns are seconds apart; break-even is about 1 paused turn in 20–30.
- **Warming:** `max_tokens: 0` requests on Anthropic, `prewarm` on OpenAI.

## 2. Tool-output reduction

**Truncation policies in real harnesses**
- **Claude Code:** Bash output capped at 30,000 characters (max 150,000); file reads at 25,000 tokens ([env vars](https://code.claude.com/docs/en/env-vars.md)).
- **OpenCode:** `MAX_LINES = 2_000`, `MAX_BYTES = 50 * 1024` (`packages/core/src/tool-output.ts` in [anomalyco/opencode](https://github.com/anomalyco/opencode)). Prune constants of 40,000 protected and 20,000 minimum tokens appear only in third-party ports; unverified upstream.
- **Codex:** head-and-tail truncation with a "…N chars truncated…" marker (`codex-rs/utils/string/src/truncate.rs`) and a `tool_output_token_limit` setting; no default is listed ([config reference](https://learn.chatgpt.com/docs/config-file/config-reference)).
- **Cursor:** writes long outputs to files and lets the agent `tail` or grep them ([blog](https://cursor.com/blog/dynamic-context-discovery)).

**Command-output compressors**
- **[rtk](https://github.com/rtk-ai/rtk):** claims "up to 90% less command output". Its README concedes that this is not a 90% cost cut and that tokens are estimated as `bytes / 4`.
- **Independent test of rtk** ([Quesma](https://quesma.com/blog/does-rtk-make-ai-coding-cheaper/), Terminal-Bench 2.1, 1,740 attempts):

  | Setup | Cost without → with RTK | Pass rate |
  |---|---|---|
  | Claude Code + Fable 5.0 | $731 → $698 | 84% → 83% |
  | OpenCode + DeepSeek V4 Pro | $51 → $54 | 71% → 69% |

  - `rtk gain` reported 89% saved, and two `head -1` calls were credited with 120.5M tokens each.
  - Terminal output was only about 7% of Fable's input and 26% of DeepSeek's.
  - DeepSeek took 18% more turns with RTK.
- **JetBrains test of rtk:** reportedly no savings and a small cost increase. Unverified (403; seen only in search summaries).
- **[headroom](https://github.com/chopratejas/headroom)** (vendor): 21–57% fewer input tokens on four scenarios. It compresses only new bytes and stores originals for retrieval.
- **[Chisle](https://github.com/JayPokale/Chisle)** (vendor, publishes its losses): oversized outputs about 48% smaller, which is 7.1% of session content.
- **[caveman](https://github.com/JuliusBrussee/caveman)** (vendor): per-file compression of about 98% became 33.2% fewer session input tokens over 54 runs. HTML got 9.9% worse.

**Other evidence**
- A [Purdue study](https://arxiv.org/abs/2609.30725) found that subsumed retrieval, repeated script generation and test re-execution affect 79–98% of tasks and account for up to 22.75% of task cost. Developer-designed skills cut cost by up to 41.73%. Structure-aware retrieval raised cost by up to 28.14%.

**Takeaway:** build bounded, recoverable outputs (cap, spill to file, failures-only test summaries) into the tool itself. Do not expect double-digit bill reductions from output filters on current models.

## 3. Tool-definition overhead

- **Tool search / `defer_loading`** ([Anthropic engineering](https://www.anthropic.com/engineering/advanced-tool-use), [docs](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool.md)):
  - 58 tools cost about 55K tokens; Anthropic saw 134K internally.
  - Context fell from about 77K to about 8.7K (85%).
  - Accuracy rose from 49% to 74% (Opus 4) and from 79.5% to 88.1% (Opus 4.5).
  - With 502 tools, run cost rose from $0.55 to $1.02 with everything loaded and stayed at $0.56 with search (cost guide).
  - Use it above roughly 10 tools or 10K tokens. Deferred tools stay out of the cached prefix.
  - A client-side implementation works on any provider: return `tool_reference` blocks, or keep a names-only list plus a "load tool" step.
- **Cursor:** MCP descriptions synced to files "reduced total agent tokens by 46.9%" in an A/B test on runs that called an MCP tool.
- **Programmatic tool calling:** 43,588 → 27,297 tokens (37%) per Anthropic engineering; the cost guide says 24% fewer input tokens with a higher score.
- **Code mode** ([Cloudflare](https://blog.cloudflare.com/code-mode-mcp/)): 1.17M tokens of tool definitions reduced to about 1,000 via `search()` and `execute()`. This is a tiktoken count of definitions, not a task benchmark.
- **Anthropic's [code execution with MCP](https://www.anthropic.com/engineering/code-execution-with-mcp):** 150,000 → 2,000 tokens (98.7%). No methodology is given, so treat it as illustrative.
- **Skills:** only name and description sit in the prompt. Claude Code advises keeping CLAUDE.md under 200 lines and preferring CLIs to MCP servers ([costs docs](https://code.claude.com/docs/en/costs)).
- **Chisle:** denying unused tools took the base request from 22.0k to 18.9k tokens.

## 4. History management

- **Observation masking vs summarisation** ([The Complexity Trap](https://arxiv.org/abs/2508.21433), SWE-bench Verified, SWE-agent, five model configurations): masking "halves cost relative to the raw agent" and matches the solve rate of LLM summarisation. A hybrid saves a further 7% over masking and 11% over summarisation.
- **Anthropic's own runs (cost guide):**
  - Context editing on a short run added 73%, because each clear rewrites the cache.
  - Compaction on a long run saved 32%; pruning saved 39%.
  - A 90K compaction trigger cost 70% less per request than the 150K default with 10K-token turns.
  - Make model or effort changes right after a compaction: $0.75 per session, against $0.95 for a mid-session change.
- **Context-editing defaults** ([docs](https://platform.claude.com/docs/en/build-with-claude/context-editing.md)): trigger at 100,000 input tokens, keep 3 tool uses; use `clear_at_least` so each cache rewrite pays for itself.
- **Compaction mechanics:** Claude Code summarises through a same-prefix fork. `/rewind` returns to an already-cached prefix. `/clear` costs nothing.
- **Sub-agents:**
  - A fresh sub-agent does not share the parent's cache; a fork does.
  - Agents use about 4x chat tokens and multi-agent systems about 15x ([Anthropic](https://www.anthropic.com/engineering/multi-agent-research-system)). Claude Code agent teams use about 7x.
  - Opus 5.5 with Sonnet 5.5 sub-agents cost about one-third less on work larger than one context window.
  - Haiku sub-agents on coding cut cost 32% and pass rate by about 4 points, roughly what lowering effort gives.
- **Hand-off files:** Manus's "restorable compression" (keep the path or URL, drop the content) and Cursor's history-as-file give a recoverable reset. Savings are not quantified.

## 5. Input shaping

- **Repo map** ([aider](https://aider.chat/docs/repomap.html)): a graph-ranked list of key symbols; `--map-tokens` "defaults to 1k tokens". No cost-per-task measurement was found.
- **Outline views:** [repomix](https://github.com/yamadashy/repomix) `--compress` extracts signatures with Tree-sitter and `--remove-comments` strips comments. No accuracy data is published.
- **Symbol navigation:** Claude Code recommends LSP go-to-definition over grep plus multiple reads; unquantified.
- **Edit format** ([aider](https://aider.chat/docs/more/edit-formats.html)): whole-file "can be slow and costly because the LLM has to return the entire file". Output costs 5x input on most current models, so search/replace edits are the default.
- **TOON** ([repo](https://github.com/toon-format/toon), vendor): 72.2% accuracy against JSON's 71.4% with 42.6% fewer tokens. Against compact JSON the saving is about 14% (2,474 vs 2,892 tokens). The README itself says compact JSON wins on deeply nested data and CSV on flat tables. One independent test reportedly had TOON last on nested data (43.1% vs JSON 50.3%); that is a secondary source and unverified.
- **Compressed prompts:** [LLMLingua](https://github.com/microsoft/LLMLingua) claims "up to 20x compression with minimal performance loss" on QA and RAG tasks. The [CAVEWOMAN paper](https://arxiv.org/abs/2606.24083) (eight models, five datasets) found input compression raises net cost by about 1.15x (up to 1.8x) while "accuracy collapses". Compressing history also rewrites the cached prefix.
- **Data files:** via Files API plus code execution, 25 of 25 correct at $0.40; pasted into the prompt, 7 of 25 at $5.01 (cost guide).

## 6. Output shaping

- **Effort (Opus 5.5, SWE-bench Pro):**

  | Effort | Solved | Cost per task |
  |---|---|---|
  | low | 87.4% | $0.11 |
  | medium | 92.8% | $0.21 |
  | high | 95.3% | $0.29 |
  | xhigh | 96.7% | $0.74 |

- **Re-run failures:** low effort first, then failures re-run at high, gave about 97% at about $0.17.
- **`max_tokens`:** a backstop only. At 16,384 it capped 85% of Fable 5.2 attempts (20.3% solved, against 77.4% at 64,000).
- **Output format:** a one-line format cost $0.49 per run; a six-section memo cost $1.40, at similar correctness.
- **Time-awareness prompt:** 60% less cost for 4.1 points on one benchmark.
- **Stop sequences:** recommended as early exits; unquantified.
- **Terse-style prompting:**
  - CAVEWOMAN reports output compression cuts cost 1.4–2.4x per model, on QA-style datasets rather than agent loops.
  - Caveman's README says `/caveman` cuts only 3% more than "be concise" at the median, and its rules cost about 1,160 input tokens.
  - [claude-token-efficient](https://github.com/drona23/claude-token-efficient) admits a net token increase on low-output exchanges.
  - Chisle reports 57% of baseline output tokens on Sonnet 5.5 with accuracy tied, measured with tools off.
- **OpenAI:** exposes `model_reasoning_effort` and `model_verbosity`; no measured numbers found.

## 7. Routing and cascades

- A stronger model at lower effort often beats a cascade. Opus 5.5 at low (87.4%, $0.12) matches Sonnet 5.5 at default (87.6%, $0.11). Fable 5.1 scored about the same as Opus 5.5 at five times the cost.
- **Advisor:** +1.7 points (edge of noise) at about 2.1x cost on coding. On one benchmark the executor consulted on 1 of 300 tasks and scored 7 points lower.
- **Cache loss:** caches are per model. "Switching from Opus to Haiku 100k tokens into a conversation can cost more than letting Opus answer." Route by sub-agent or between sessions, never mid-conversation.
- **Caveman "Auto" router** (vendor, interim, ±7 points): 43.7% vs 44.3% score at $3.59 vs $7.83.
- **Safe uses of a cheap model:** titles, summaries and search sub-agents with their own prefix.

## 8. Measurement

**Track**
- Cost per solved task, counting failed attempts.
- Cache-read share of input.
- Turns per task.
- Tail spend: two of 20 problems carried 43% of spend.
- Output and reasoning share: 56–57% of DeepSeek's bill in the Quesma run.

**Public numbers**
- Claude Code averages about $13 per developer per active day, and under $30 for 90% of users.

**How savings claims mislead**
1. Counting bytes removed, not billed tokens.
2. Ignoring that cached input is 94–98% of tokens but only about 26–30% of the bill.
3. Ignoring extra turns and retries.
4. Per-file ratios presented as session savings.
5. Counterfactuals against output that was never emitted.
6. Tools-off or single-run benchmarks.
7. No pass-rate reporting.

## (a) Top 15 to build in, by savings-to-risk

| # | Technique | Expected saving | Source |
|---|---|---|---|
| 1 | Stable-prefix caching (append-only history, per-provider layout) | 1.7–7.3x cost | Anthropic cost guide |
| 2 | Cache hit-rate telemetry and breaker diagnosis (alert below ~80%) | protects #1 | cost guide, Claude Code |
| 3 | State changes as messages, never system or tool edits | avoids 25–50x per edit | cost guide |
| 4 | Deferred tools / tool search above ~10K tokens of schemas | ~85% of definition tokens; 46.9% total on MCP runs | Anthropic, Cursor |
| 5 | Diff/search-replace edits | large on output; unquantified | aider |
| 6 | Bounded tool output with spill-to-file | prevents blowups; small on average | OpenCode, Cursor, Quesma |
| 7 | Observation masking before LLM summaries, in large infrequent batches | ~50% vs raw agent | Complexity Trap |
| 8 | Cache-safe compaction with a tuned trigger | 32% on long runs; 70% per request at 90K trigger | cost guide |
| 9 | Per-route effort control | 25–50% per step down, for single-digit points | cost guide |
| 10 | Escalate-on-failure (low, then high) | ~40% vs all-high | cost guide |
| 11 | Model or effort switches only at compaction or session boundaries | $0.75 vs $0.95 per session | cost guide |
| 12 | Sub-agents and forks for bulky side work | ~33% on work beyond one window; negative otherwise | cost guide |
| 13 | Code-mode or programmatic tool calling for chained calls | 24–37% input | Anthropic |
| 14 | Skills and progressive disclosure; small instruction files | up to 41.73% | Purdue |
| 15 | TTL policy and keep-alive (5 min default, 1 h or ping for human pauses) | 8–19% | cost guide |

Batch or flex mode for unattended runs adds a further 50% and stacks with caching.

## (b) Popular but weak or negative evidence

- **Command-output filters as cost savers (rtk-style):** -5% to +17% in the one independent benchmark read.
- **LLMLingua-style input compression:** net cost up and accuracy down in CAVEWOMAN; it also breaks the cache.
- **Caveman-style rule files:** marginal over "be concise", with a recurring input cost.
- **TOON for general payloads:** helps only on uniform arrays.
- **Tight `max_tokens` caps:** no improvement in cost per solved task.
- **Per-turn context editing:** +73% on a short run.
- **Advisor patterns and mid-conversation cascades:** at or below the single-model effort curve.
- **Multi-agent teams for coding:** 7–15x tokens.
- **Structure-aware retrieval add-ons:** up to +28.14% cost.
- **Comment and whitespace stripping:** no evidence found.

## (c) Per-provider cache cheat sheet

| Provider | Mode | Minimum | TTL | Write | Read | Hit field |
|---|---|---|---|---|---|---|
| Anthropic | explicit (max 4) or top-level auto | 512–4,096 by model | 5 min / 1 h | 1.25x / 2x | 0.1x (0.05x, 0.025x on newest) | `cache_read_input_tokens` |
| OpenAI | auto; explicit on 5.6+; `prompt_cache_key`, ~15 RPM per key | 1,024 | 30 min / 12 h (older: in-memory / 24 h) | 1.25x / 2.5x (older: free) | 0.1x (0.05x on 6.1) | `cached_tokens` |
| DeepSeek | auto, best-effort | none stated | hours to days | free | ~0.02–0.03x | `prompt_cache_hit_tokens` |
| Gemini | implicit; explicit on ≤3.x only | 4,096 (2,048 on 2.5) | not stated | free implicit; storage per hour explicit | 0.05–0.1x | `total_cached_tokens` |
| OpenAI-compatible servers | assume automatic prefix cache | unknown | unknown | unknown | unknown | probe by sending the same request twice |

Universal rules: never reorder tools, never edit history, and pin model and effort for the whole session.
