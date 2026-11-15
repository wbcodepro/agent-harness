# Unified harness research: source register

Research date: **15 November 2026**. See the [report](UNIFIED-HARNESS-RESEARCH-2026-11-15.md), [roadmap](UNIFIED-HARNESS-ROADMAP-2026-11-15.md), and [JSON evidence manifest](unified-harness-sources-2026-11-15.json).

This is a curated survey of **56 verified GitHub repositories**, not an exhaustive census. Metadata and READMEs were retrieved at recorded default-branch commits. Inspection depth: **5 C**, **17 D**, **34 I**.

- **I — Inventory:** repository identity, metadata and README retrieved; no detailed implementation review.
- **D — Documentation:** selected README/architecture text or official behavior documentation reviewed.
- **C — Code excerpts:** selected implementation paths examined, in addition to documentation. This is not a complete code or security audit.

No project was installed or benchmarked for this survey. Repository activity and archive status are observations as of the research date. Default-branch commits are research pins, not recommended release versions. All performance statements in the report are either attributed author results, explicitly hypothetical calculations, or proposed targets.

The license column is **GitHub-detected root metadata**, not a distribution clearance. `Unresolved` includes missing metadata and `NOASSERTION`. Cua, OmniParser, AutoGen and other mixed repositories require inspection of the particular component, dependency, weight and dataset being shipped.

Identity updates observed: `badlogic/pi-mono` resolves to `earendil-works/pi`; `block/goose` to `aaif-goose/goose`; `OpenInterpreter/open-interpreter` to `openinterpreter/openinterpreter`. The initial `FoundationAgents/OpenHands` lookup failed; `OpenHands/OpenHands` was found and verified separately. UI-TARS Desktop and AgentGPT were archived.

## Harnesses

| Repository / pinned README | Depth | Commit | Last push (UTC) | Root license / status | Relevance or decision |
|---|---|---|---|---|---|
| [openai/codex](https://github.com/openai/codex/blob/81b995a6c9a4c978f399775cfa5ce50702368749/README.md) | C | [81b995a6c9](https://github.com/openai/codex/commit/81b995a6c9a4c978f399775cfa5ce50702368749) | 2026-11-14 | Apache-2.0; not archived | Study runtime/client and tool boundaries; retain hame core. |
| [anthropics/claude-code](https://github.com/anthropics/claude-code/blob/546014d4b6bd63ad6046ec6718af46ec01ea5658/README.md) | D | [546014d4b6](https://github.com/anthropics/claude-code/commit/546014d4b6bd63ad6046ec6718af46ec01ea5658) | 2026-11-14 | Unresolved; not archived | Use official behavior and caching documentation; do not infer access to all product internals. |
| [deepseek-ai/deepseek-harness](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/README.md) | D | [39163dd30f](https://github.com/deepseek-ai/deepseek-harness/commit/39163dd30f703a79079e2d2446545cff3c453738) | 2026-11-13 | MIT; not archived | Study plugin lifecycle and adapter accounting; developer-preview compatibility risk. |
| [alibaba/open-code-review](https://github.com/alibaba/open-code-review/blob/d77d70b371c1b5526836d162029d6fb85e3ce4e0/README.md) | C | [d77d70b371](https://github.com/alibaba/open-code-review/commit/d77d70b371c1b5526836d162029d6fb85e3ce4e0) | 2026-11-14 | Apache-2.0; not archived | Candidate for targeted repository review with structured evidence. |
| [anomalyco/opencode](https://github.com/anomalyco/opencode/blob/38bd73789a43bf043599d6196af96583228035d7/README.md) | I | [38bd73789a](https://github.com/anomalyco/opencode/commit/38bd73789a43bf043599d6196af96583228035d7) | 2026-11-14 | MIT; not archived | Provider-neutral coding harness comparator; no runtime benchmark in this review. |
| [earendil-works/pi](https://github.com/earendil-works/pi/blob/df1a6d8c83d5458003103b7a5dc6cb9b3d966fa2/README.md) | D | [df1a6d8c83](https://github.com/earendil-works/pi/commit/df1a6d8c83d5458003103b7a5dc6cb9b3d966fa2) | 2026-11-14 | MIT; not archived | Study minimal extensible core; confinement remains a hame responsibility. |
| [aaif-goose/goose](https://github.com/aaif-goose/goose/blob/2365bc5e898a38133c373eef2d1357416acd66e2/README.md) | I | [2365bc5e89](https://github.com/aaif-goose/goose/commit/2365bc5e898a38133c373eef2d1357416acd66e2) | 2026-11-14 | Apache-2.0; not archived | Extensible local agent comparator; formerly under block/goose. |
| [google-gemini/gemini-cli](https://github.com/google-gemini/gemini-cli/blob/4ed06ed0ddc523a6d356a4ad35ddfea331041888/README.md) | I | [4ed06ed0dd](https://github.com/google-gemini/gemini-cli/commit/4ed06ed0ddc523a6d356a4ad35ddfea331041888) | 2026-11-14 | Apache-2.0; not archived | Coding-agent integration and evaluation comparator. |
| [SWE-agent/SWE-agent](https://github.com/SWE-agent/SWE-agent/blob/3ea751c087f32b16e039a2233dd6eefecef325d5/README.md) | I | [3ea751c087](https://github.com/SWE-agent/SWE-agent/commit/3ea751c087f32b16e039a2233dd6eefecef325d5) | 2026-11-09 | MIT; not archived | Agent-computer interface and coding-evaluation reference. |
| [SWE-agent/mini-swe-agent](https://github.com/SWE-agent/mini-swe-agent/blob/04d809ceab9df28f9adaed044884180159172930/README.md) | I | [04d809ceab](https://github.com/SWE-agent/mini-swe-agent/commit/04d809ceab9df28f9adaed044884180159172930) | 2026-11-09 | MIT; not archived | Simple-loop comparator to test whether added orchestration pays for itself. |
| [openinterpreter/openinterpreter](https://github.com/openinterpreter/openinterpreter/blob/f09c887041133f52da1155e27d29edd8fa1f516e/README.md) | I | [f09c887041](https://github.com/openinterpreter/openinterpreter/commit/f09c887041133f52da1155e27d29edd8fa1f516e) | 2026-11-14 | Apache-2.0; not archived | Program-execution agent lineage; current project differs from older descriptions. |
| [Aider-AI/aider](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/README.md) | I | [5dc9490bb3](https://github.com/Aider-AI/aider/commit/5dc9490bb35f9729ef2c95d00a19ccd30c26339c) | 2026-05-22 | Apache-2.0; not archived | Editing and repository-context reference; current metadata does not mark it archived. |
| [OpenHands/OpenHands](https://github.com/OpenHands/OpenHands/blob/4bbea81dc73aac76857307000bf9c91329c5f3c3/README.md) | I | [4bbea81dc7](https://github.com/OpenHands/OpenHands/commit/4bbea81dc73aac76857307000bf9c91329c5f3c3) | 2026-11-14 | MIT; not archived | Runtime and coding-agent ecosystem comparator; canonical repository verified separately. |

## Browser execution

| Repository / pinned README | Depth | Commit | Last push (UTC) | Root license / status | Relevance or decision |
|---|---|---|---|---|---|
| [vercel-labs/agent-browser](https://github.com/vercel-labs/agent-browser/blob/e0c121a96d3a5412e2078e0c8eba3f4d2db2b616/README.md) | D | [e0c121a96d](https://github.com/vercel-labs/agent-browser/commit/e0c121a96d3a5412e2078e0c8eba3f4d2db2b616) | 2026-11-14 | Apache-2.0; not archived | Primary candidate: compact semantic snapshots and a persistent browser daemon. |
| [browser-use/browser-use](https://github.com/browser-use/browser-use/blob/e717df1839a2d7368b108de6694f01e9c659b0de/README.md) | D | [e717df1839](https://github.com/browser-use/browser-use/commit/e717df1839a2d7368b108de6694f01e9c659b0de) | 2026-11-14 | MIT; not archived | Current README describes persistent Python REPL/CDP execution; pin the breaking API generation. |
| [browserbase/stagehand](https://github.com/browserbase/stagehand/blob/6f2686d792b7f5836b04754e0f6a44db39928592/README.md) | D | [6f2686d792](https://github.com/browserbase/stagehand/commit/6f2686d792b7f5836b04754e0f6a44db39928592) | 2026-11-14 | MIT; not archived | Current v4 architecture: execution near browser, trimmed context and caching; internal inference costs matter. |
| [microsoft/playwright-mcp](https://github.com/microsoft/playwright-mcp/blob/78eca85361efb545a4c0f59b6140aef0721c5412/README.md) | D | [78eca85361](https://github.com/microsoft/playwright-mcp/commit/78eca85361efb545a4c0f59b6140aef0721c5412) | 2026-11-13 | Apache-2.0; not archived | Structured browser observations through MCP; compare with CLI/script exposure. |
| [microsoft/playwright](https://github.com/microsoft/playwright/blob/b4d016aadc5cbd2b898a284307975fe73e05483d/README.md) | D | [b4d016aadc](https://github.com/microsoft/playwright/commit/b4d016aadc5cbd2b898a284307975fe73e05483d) | 2026-11-14 | Apache-2.0; not archived | Execution and actionability baseline; official actionability documentation reviewed. |
| [microsoft/playwright-cli](https://github.com/microsoft/playwright-cli/blob/b21724888b8100b05ef8dee3a92f2bb03bf4dcde/README.md) | I | [b21724888b](https://github.com/microsoft/playwright-cli/commit/b21724888b8100b05ef8dee3a92f2bb03bf4dcde) | 2026-11-11 | Apache-2.0; not archived | CLI alternative to include in the browser adapter comparison. |
| [browser-use/browser-harness](https://github.com/browser-use/browser-harness/blob/f81992a129b1d125ee3fce0a1c48ef1e7b95dad7/README.md) | I | [f81992a129](https://github.com/browser-use/browser-harness/commit/f81992a129b1d125ee3fce0a1c48ef1e7b95dad7) | 2026-11-14 | MIT; not archived | Browser Use helper layer; candidate for a narrower integration than the full agent. |

## Computer control and perception

| Repository / pinned README | Depth | Commit | Last push (UTC) | Root license / status | Relevance or decision |
|---|---|---|---|---|---|
| [vercel-labs/agent-desktop](https://github.com/vercel-labs/agent-desktop/blob/46d2adf885002633b77d5a9956c9464b24f61431/README.md) | C | [46d2adf885](https://github.com/vercel-labs/agent-desktop/commit/46d2adf885002633b77d5a9956c9464b24f61431) | 2026-11-14 | Apache-2.0; not archived | Primary native semantic-control candidate; inspected ref execution and preflight tracing. |
| [anthropics/claude-quickstarts](https://github.com/anthropics/claude-quickstarts/blob/4a4224bdf0e9cfb3332167b69c90457403813625/README.md) | C | [4a4224bdf0](https://github.com/anthropics/claude-quickstarts/commit/4a4224bdf0e9cfb3332167b69c90457403813625) | 2026-11-06 | MIT; not archived | Reference screenshot loop, batching and image transforms; best-practices example is macOS-specific. |
| [microsoft/UFO](https://github.com/microsoft/UFO/blob/eae3c0a8d168c47cb10df0652e293e092eda1078/README.md) | D | [eae3c0a8d1](https://github.com/microsoft/UFO/commit/eae3c0a8d168c47cb10df0652e293e092eda1078) | 2026-10-30 | MIT; not archived | Application/device orchestration reference; broad orchestration is optional. |
| [microsoft/OmniParser](https://github.com/microsoft/OmniParser/blob/354021201345a96178360b28733573e27269f2de/README.md) | D | [3540212013](https://github.com/microsoft/OmniParser/commit/354021201345a96178360b28733573e27269f2de) | 2026-11-14 | CC-BY-4.0; not archived | Local screen parsing candidate; component and model terms differ across versions. |
| [bytedance/UI-TARS-desktop](https://github.com/bytedance/UI-TARS-desktop/blob/3ac2cb8d946d78d5ff92ad17b318eaafa18c178c/README.md) | D | [3ac2cb8d94](https://github.com/bytedance/UI-TARS-desktop/commit/3ac2cb8d946d78d5ff92ad17b318eaafa18c178c) | 2026-10-09 | Apache-2.0; archived | Visual-agent reference; archived at inspection, so not a default maintained dependency. |
| [simular-ai/Agent-S](https://github.com/simular-ai/Agent-S/blob/0fff0e634869dab72027435b4586e62300185d41/README.md) | D | [0fff0e6348](https://github.com/simular-ai/Agent-S/commit/0fff0e634869dab72027435b4586e62300185d41) | 2026-11-13 | Apache-2.0; not archived | Agent S4 interface-evolution research; author-reported results require local reproduction. |
| [trycua/cua](https://github.com/trycua/cua/blob/153c0052bac22bb843378b31f3f7a65ac63bf074/README.md) | D | [153c0052ba](https://github.com/trycua/cua/commit/153c0052bac22bb843378b31f3f7a65ac63bf074) | 2026-11-14 | MIT; not archived | Drivers and isolated environments; mixed component terms and deprecated packages require exact selection. |
| [microsoft/Hybrid-Computer-Use](https://github.com/microsoft/Hybrid-Computer-Use/blob/697d3fbda04cb322915866bab1aad79c726a61e3/README.md) | C | [697d3fbda0](https://github.com/microsoft/Hybrid-Computer-Use/commit/697d3fbda04cb322915866bab1aad79c726a61e3) | 2026-11-02 | MIT; not archived | Closest hybrid-interface comparator; inspected selected batch binding/planning code. |

## Evaluation

| Repository / pinned README | Depth | Commit | Last push (UTC) | Root license / status | Relevance or decision |
|---|---|---|---|---|---|
| [xlang-ai/OSWorld](https://github.com/xlang-ai/OSWorld/blob/5123bfdbbc34b947bcffcff564abc62d0ab7072d/README.md) | I | [5123bfdbbc](https://github.com/xlang-ai/OSWorld/commit/5123bfdbbc34b947bcffcff564abc62d0ab7072d) | 2026-11-14 | Apache-2.0; not archived | Desktop evaluation environment; fix task revision and allowed channels. |
| [WukLab/osworld-human](https://github.com/WukLab/osworld-human/blob/deff1a7cd8940f6040a895593097fc3c5511f36b/README.md) | D | [deff1a7cd8](https://github.com/WukLab/osworld-human/commit/deff1a7cd8940f6040a895593097fc3c5511f36b) | 2026-05-17 | Unresolved; not archived | Latency and human-efficiency reference; older README numbers differ from the revised paper. |
| [ServiceNow/BrowserGym](https://github.com/ServiceNow/BrowserGym/blob/27a90ce815098f43414e93491e06907af468e45b/README.md) | I | [27a90ce815](https://github.com/ServiceNow/BrowserGym/commit/27a90ce815098f43414e93491e06907af468e45b) | 2026-11-09 | Unresolved; not archived | Browser environment integration and task-evaluation reference. |
| [ServiceNow/WorkArena](https://github.com/ServiceNow/WorkArena/blob/e885130962f61fb64ac305497996a46cfc7d18f0/README.md) | I | [e885130962](https://github.com/ServiceNow/WorkArena/commit/e885130962f61fb64ac305497996a46cfc7d18f0) | 2026-09-28 | Unresolved; not archived | Enterprise browser-workflow evaluation reference. |
| [web-arena-x/webarena](https://github.com/web-arena-x/webarena/blob/dce04686a56253aefba7b18a4fa0937cf1dc987b/README.md) | I | [dce04686a5](https://github.com/web-arena-x/webarena/commit/dce04686a56253aefba7b18a4fa0937cf1dc987b) | 2025-11-26 | Apache-2.0; not archived | Resettable web-task evaluation reference. |
| [harbor-framework/harbor](https://github.com/harbor-framework/harbor/blob/ec6aeab7fe0eb51df52b4e72c524daf5ea9cc5f4/README.md) | I | [ec6aeab7fe](https://github.com/harbor-framework/harbor/commit/ec6aeab7fe0eb51df52b4e72c524daf5ea9cc5f4) | 2026-11-14 | Apache-2.0; not archived | External evaluation infrastructure candidate; retain a product-specific pilot suite. |
| [harbor-framework/terminal-bench](https://github.com/harbor-framework/terminal-bench/blob/348004f0ed04292f756a7ff9d542e95374d8aa34/README.md) | I | [348004f0ed](https://github.com/harbor-framework/terminal-bench/commit/348004f0ed04292f756a7ff9d542e95374d8aa34) | 2026-11-14 | Apache-2.0; not archived | Terminal-task evaluation reference; not a complete desktop benchmark. |
| [SWE-bench/SWE-bench](https://github.com/SWE-bench/SWE-bench/blob/02e7a74ffd0b707aab73d203fe87bdc7c76afc8e/README.md) | I | [02e7a74ffd](https://github.com/SWE-bench/SWE-bench/commit/02e7a74ffd0b707aab73d203fe87bdc7c76afc8e) | 2026-09-18 | MIT; not archived | Coding regression reference; do not equate its scores with desktop task success. |
| [xlang-ai/OSWorld-V2](https://github.com/xlang-ai/OSWorld-V2/blob/3f7bee39938c7fc707b6ce81a682066f9ff813cc/README.md) | I | [3f7bee3993](https://github.com/xlang-ai/OSWorld-V2/commit/3f7bee39938c7fc707b6ce81a682066f9ff813cc) | 2026-11-13 | Apache-2.0; not archived | Long-workflow evaluation repository; distinguish release and scoring versions. |

## Protocols

| Repository / pinned README | Depth | Commit | Last push (UTC) | Root license / status | Relevance or decision |
|---|---|---|---|---|---|
| [modelcontextprotocol/modelcontextprotocol](https://github.com/modelcontextprotocol/modelcontextprotocol/blob/0175d20f8b2c49e0402c37c37ed39ae2be47277e/README.md) | D | [0175d20f8b](https://github.com/modelcontextprotocol/modelcontextprotocol/commit/0175d20f8b2c49e0402c37c37ed39ae2be47277e) | 2026-11-14 | Unresolved; not archived | Tool integration standard; current version and backward compatibility checked in official docs. |
| [agentclientprotocol/agent-client-protocol](https://github.com/agentclientprotocol/agent-client-protocol/blob/13b765828f961812f0c78ae577220e994209fa2f/README.md) | I | [13b765828f](https://github.com/agentclientprotocol/agent-client-protocol/commit/13b765828f961812f0c78ae577220e994209fa2f) | 2026-11-14 | Apache-2.0; not archived | Editor-agent boundary already implemented in hame. |
| [agentskills/agentskills](https://github.com/agentskills/agentskills/blob/83021e2be457a9b6679148719c1f6074f0ad975e/README.md) | I | [83021e2be4](https://github.com/agentskills/agentskills/commit/83021e2be457a9b6679148719c1f6074f0ad975e) | 2026-11-04 | Apache-2.0; not archived | Portable skill packaging; instructions do not grant execution authority. |
| [agentsmd/agents.md](https://github.com/agentsmd/agents.md/blob/d001185d792eb6402a58e4cbef1c228b309ec25d/README.md) | I | [d001185d79](https://github.com/agentsmd/agents.md/commit/d001185d792eb6402a58e4cbef1c228b309ec25d) | 2026-09-10 | MIT; not archived | Repository instruction convention; preserve provenance and scope. |
| [a2aproject/A2A](https://github.com/a2aproject/A2A/blob/4d7cda4b1754ed410e41bf3be7ad9ebc62afe3d3/README.md) | I | [4d7cda4b17](https://github.com/a2aproject/A2A/commit/4d7cda4b1754ed410e41bf3be7ad9ebc62afe3d3) | 2026-11-13 | Apache-2.0; not archived | Optional external-agent task delegation boundary. |

## Workflow runtimes

| Repository / pinned README | Depth | Commit | Last push (UTC) | Root license / status | Relevance or decision |
|---|---|---|---|---|---|
| [langchain-ai/langgraph](https://github.com/langchain-ai/langgraph/blob/08a2016953789a4f579b4d36bd1967941ec62012/README.md) | D | [08a2016953](https://github.com/langchain-ai/langgraph/commit/08a2016953789a4f579b4d36bd1967941ec62012) | 2026-11-14 | MIT; not archived | State persistence and checkpoint/store separation reference. |
| [microsoft/autogen](https://github.com/microsoft/autogen/blob/8390985108cb1e3881f5f7297765758d8ad2540c/README.md) | I | [8390985108](https://github.com/microsoft/autogen/commit/8390985108cb1e3881f5f7297765758d8ad2540c) | 2026-11-12 | CC-BY-4.0; not archived | Multi-agent orchestration reference; repository metadata is not a complete component license assessment. |
| [crewAIInc/crewAI](https://github.com/crewAIInc/crewAI/blob/d7dfa0ef5ade793bd48ff7357c6eb45a45294371/README.md) | I | [d7dfa0ef5a](https://github.com/crewAIInc/crewAI/commit/d7dfa0ef5ade793bd48ff7357c6eb45a45294371) | 2026-11-13 | MIT; not archived | Role/workflow orchestration comparator; measure coordination overhead before adoption. |
| [langchain-ai/deepagents](https://github.com/langchain-ai/deepagents/blob/a93aed4bca3a5e4d45bff393e16092222767e6c3/README.md) | I | [a93aed4bca](https://github.com/langchain-ai/deepagents/commit/a93aed4bca3a5e4d45bff393e16092222767e6c3) | 2026-11-14 | MIT; not archived | Higher-level agent runtime comparator; avoid duplicated control loops without a defined need. |

## Historical foundations

| Repository / pinned README | Depth | Commit | Last push (UTC) | Root license / status | Relevance or decision |
|---|---|---|---|---|---|
| [Significant-Gravitas/AutoGPT](https://github.com/Significant-Gravitas/AutoGPT/blob/ac4e88888d3838fd5790dff2a85845a654d05024/README.md) | I | [ac4e88888d](https://github.com/Significant-Gravitas/AutoGPT/commit/ac4e88888d3838fd5790dff2a85845a654d05024) | 2026-11-14 | Unresolved; not archived | Early modern-agent lineage; current platform and licensing are broader than early demos. |
| [yoheinakajima/babyagi](https://github.com/yoheinakajima/babyagi/blob/fa8930ebe72a82e5ad57b356e7cbec96290e5bb2/README.md) | I | [fa8930ebe7](https://github.com/yoheinakajima/babyagi/commit/fa8930ebe72a82e5ad57b356e7cbec96290e5bb2) | 2026-01-31 | Unresolved; not archived | Task-management agent lineage; current repository is not proof of the original implementation details. |
| [reworkd/AgentGPT](https://github.com/reworkd/AgentGPT/blob/18b073ab05b2902e1d052c3d2799786d8623b5e5/README.md) | I | [18b073ab05](https://github.com/reworkd/AgentGPT/commit/18b073ab05b2902e1d052c3d2799786d8623b5e5) | 2025-04-29 | GPL-3.0; archived | Early browser-hosted agent lineage; archived at inspection. |
| [MineDojo/Voyager](https://github.com/MineDojo/Voyager/blob/55e45a880755d0c8c66ca7fb5fe7962ac8974f89/README.md) | D | [55e45a8807](https://github.com/MineDojo/Voyager/commit/55e45a880755d0c8c66ca7fb5fe7962ac8974f89) | 2024-04-03 | MIT; not archived | Executable skill-library precedent, grounded in a different environment from desktop work. |

## Application tools

| Repository / pinned README | Depth | Commit | Last push (UTC) | Root license / status | Relevance or decision |
|---|---|---|---|---|---|
| [googleworkspace/cli](https://github.com/googleworkspace/cli/blob/cd3697e07eb520056dcf142d0c7439bbb9ba32e9/README.md) | I | [cd3697e07e](https://github.com/googleworkspace/cli/commit/cd3697e07eb520056dcf142d0c7439bbb9ba32e9) | 2026-11-03 | Apache-2.0; not archived | Candidate for semantic service operations that can replace long GUI sequences. |
| [microsoft/markitdown](https://github.com/microsoft/markitdown/blob/ed178952a6672abdfd35b5f9e76a80b6cefc086e/README.md) | I | [ed178952a6](https://github.com/microsoft/markitdown/commit/ed178952a6672abdfd35b5f9e76a80b6cefc086e) | 2026-11-07 | MIT; not archived | Document-to-text extraction candidate; extraction alone does not verify visual formatting. |

## Isolation and observability

| Repository / pinned README | Depth | Commit | Last push (UTC) | Root license / status | Relevance or decision |
|---|---|---|---|---|---|
| [firecracker-microvm/firecracker](https://github.com/firecracker-microvm/firecracker/blob/cc88013eadcf432da1b37d47d445b7e8bcc3046e/README.md) | I | [cc88013ead](https://github.com/firecracker-microvm/firecracker/commit/cc88013eadcf432da1b37d47d445b7e8bcc3046e) | 2026-11-13 | Apache-2.0; not archived | Linux microVM infrastructure reference; not a universal host-desktop isolation solution. |
| [e2b-dev/E2B](https://github.com/e2b-dev/E2B/blob/6dc8d5a10aa5db8dc8cf9c4d95fc769d886197fd/README.md) | I | [6dc8d5a10a](https://github.com/e2b-dev/E2B/commit/6dc8d5a10aa5db8dc8cf9c4d95fc769d886197fd) | 2026-11-14 | Apache-2.0; not archived | Remote execution environment candidate; include setup and compute costs. |
| [langfuse/langfuse](https://github.com/langfuse/langfuse/blob/48a31810e1c6e6a4a589b1eacc5eecc7960c9b87/README.md) | I | [48a31810e1](https://github.com/langfuse/langfuse/commit/48a31810e1c6e6a4a589b1eacc5eecc7960c9b87) | 2026-11-14 | Unresolved; not archived | Optional telemetry/evaluation backend; keep hame trace format independent. |
| [open-telemetry/semantic-conventions](https://github.com/open-telemetry/semantic-conventions/blob/45b690d4a7af791c8c21956c1c8d78ec29295ea9/README.md) | I | [45b690d4a7](https://github.com/open-telemetry/semantic-conventions/commit/45b690d4a7af791c8c21956c1c8d78ec29295ea9) | 2026-11-13 | Apache-2.0; not archived | Versioned telemetry export reference; stabilize hame internal fields first. |

## Selected pinned files

The manifest retains content hashes for these files. Retrieved-only files are listed to distinguish collection from substantive inspection.

| File | Inspection |
|---|---|
| [openai/codex: codex-rs/app-server/README.md](https://github.com/openai/codex/blob/81b995a6c9a4c978f399775cfa5ce50702368749/codex-rs/app-server/README.md) | documentation excerpts |
| [openai/codex: codex-rs/core/src/tools/registry.rs](https://github.com/openai/codex/blob/81b995a6c9a4c978f399775cfa5ce50702368749/codex-rs/core/src/tools/registry.rs) | retrieved reference; not substantively reviewed |
| [deepseek-ai/deepseek-harness: docs/architecture.md](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/docs/architecture.md) | documentation excerpts |
| [deepseek-ai/deepseek-harness: packages/experimental/browser-use-stagehand-native/README.md](https://github.com/deepseek-ai/deepseek-harness/blob/39163dd30f703a79079e2d2446545cff3c453738/packages/experimental/browser-use-stagehand-native/README.md) | documentation excerpts |
| [microsoft/Hybrid-Computer-Use: computer-use/internal/core/ax_batch.go](https://github.com/microsoft/Hybrid-Computer-Use/blob/697d3fbda04cb322915866bab1aad79c726a61e3/computer-use/internal/core/ax_batch.go) | implementation excerpts |
| [microsoft/Hybrid-Computer-Use: computer-use/internal/core/ax_batch_plan.go](https://github.com/microsoft/Hybrid-Computer-Use/blob/697d3fbda04cb322915866bab1aad79c726a61e3/computer-use/internal/core/ax_batch_plan.go) | implementation excerpts |
| [microsoft/Hybrid-Computer-Use: computer-use/internal/core/ax_semantic_diff.go](https://github.com/microsoft/Hybrid-Computer-Use/blob/697d3fbda04cb322915866bab1aad79c726a61e3/computer-use/internal/core/ax_semantic_diff.go) | retrieved reference; not substantively reviewed |
| [microsoft/Hybrid-Computer-Use: docs/PLATFORMS.md](https://github.com/microsoft/Hybrid-Computer-Use/blob/697d3fbda04cb322915866bab1aad79c726a61e3/docs/PLATFORMS.md) | retrieved reference; not substantively reviewed |
| [vercel-labs/agent-desktop: crates/core/src/commands/execute_by_ref.rs](https://github.com/vercel-labs/agent-desktop/blob/46d2adf885002633b77d5a9956c9464b24f61431/crates/core/src/commands/execute_by_ref.rs) | implementation read |
| [vercel-labs/agent-desktop: crates/core/src/ref_action/ref_action_actionability.rs](https://github.com/vercel-labs/agent-desktop/blob/46d2adf885002633b77d5a9956c9464b24f61431/crates/core/src/ref_action/ref_action_actionability.rs) | implementation read |
| [anthropics/claude-quickstarts: computer-use-best-practices/README.md](https://github.com/anthropics/claude-quickstarts/blob/4a4224bdf0e9cfb3332167b69c90457403813625/computer-use-best-practices/README.md) | documentation excerpts |
| [anthropics/claude-quickstarts: computer-use-best-practices/computer_use/image.py](https://github.com/anthropics/claude-quickstarts/blob/4a4224bdf0e9cfb3332167b69c90457403813625/computer-use-best-practices/computer_use/image.py) | implementation read |
| [alibaba/open-code-review: internal/agent/agent.go](https://github.com/alibaba/open-code-review/blob/d77d70b371c1b5526836d162029d6fb85e3ce4e0/internal/agent/agent.go) | implementation excerpts |
| [simular-ai/Agent-S: agent_s4/README.md](https://github.com/simular-ai/Agent-S/blob/0fff0e634869dab72027435b4586e62300185d41/agent_s4/README.md) | documentation excerpts |

## Papers and official documentation

| Primary source | Use and limitation |
|---|---|
| [ReAct](https://arxiv.org/abs/2210.03629) | Modern reasoning/action-loop foundation; not a desktop reliability result. |
| [OSWorld](https://arxiv.org/abs/2404.07972) | Original real-computer environment benchmark. |
| [OSWorld-Human v2](https://arxiv.org/abs/2506.16042v2) | Latency analysis; May 2026 revision differs from older README summary. |
| [OSWorld 2.0 v2](https://arxiv.org/abs/2606.29537v2) | Long-horizon tasks and distinct binary/partial scores. |
| [The Complexity Trap v3](https://arxiv.org/abs/2508.21433v3) | Observation masking versus summarization in coding-agent settings. |
| [Evolving Agent Interfaces / Agent S4 v1](https://arxiv.org/abs/2610.28601v1) | Offline interface evolution; author-reported benchmark results, not reproduced here. |
| [Hybrid Computer Use v1](https://arxiv.org/abs/2610.41432v1) | Combined programmatic, semantic and visual interfaces; author-reported comparisons. |
| [Building effective agents](https://www.anthropic.com/research/building-effective-agents) | Historical workflow/agent distinction; page itself notes that tooling has evolved. |
| [Effective context engineering](https://www.anthropic.com/research/effective-context-engineering-for-ai-agents) | Context selection and state management principles. |
| [Code execution with MCP](https://www.anthropic.com/research/code-execution-with-mcp) | Progressive tool discovery and local processing of intermediate results. |
| [Advanced tool use](https://www.anthropic.com/research/advanced-tool-use) | Programmatic calling and deferred discovery; vendor evidence. |
| [How Claude Code works](https://code.claude.com/docs/en/how-claude-code-works) | Documented behavior, not reverse-engineered product internals. |
| [Claude Code prompt caching](https://code.claude.com/docs/en/prompt-caching) | Cache organization and invalidation behavior. |
| [MCP versioning](https://modelcontextprotocol.io/docs/2026-07-28/learn/versioning) | Current revision and backward-compatibility mechanism at inspection. |
| [Playwright actionability](https://playwright.dev/docs/actionability) | Local preconditions and waiting behavior for browser actions. |
| [Apple ScreenCaptureKit](https://developer.apple.com/documentation/ScreenCaptureKit) | Native capture capabilities and platform permission boundary. |
| [Microsoft UI Automation](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-uiautomationoverview) | Native semantic UI access on Windows. |
| [XDG RemoteDesktop portal](https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.RemoteDesktop.html) | Linux remote-input and capture-session lifecycle. |
| [LangGraph persistence](https://docs.langchain.com/oss/python/langgraph/persistence) | Thread checkpoints versus cross-thread stores. |

## Local source anchors

Local baseline: `cc2ac8278ff85e7938ae169e3825a616742f7426`. Inspection was read-only. The pre-existing untracked `ACP_SMOKE_TEST_20261114.md` was left untouched.

| Source | Finding used in the report |
|---|---|
| [Session model](../../crates/hame-session/src/model.rs) | Event log and usage fields; no image block in the examined content enum |
| [Recovery](../../crates/hame-session/src/recover.rs) | Distinguishes an interrupted running call from one that never started |
| [Tool types](../../crates/hame-tools/src/lib.rs) | Text-only ToolOutput; three effect classes; execution/concurrency boundaries |
| [MCP adapter](../../crates/hame-ext/src/mcp.rs) | Non-text content replaced by omission markers; protocol compatibility paths |
| [Script execution](../../crates/hame-ext/src/script.rs) | New Python process per script; nested tool calls pass through policy pipeline |
| [Runtime assembly](../../crates/hame-app/src/lib.rs) | Deferred tool/script wiring and agent construction |
| [Deferred discovery](../../crates/hame-agent/src/deferred.rs) | Search/call surface for tools outside the main catalog |
| [Provider adapters](../../crates/hame-llm/src/lib.rs) | Request abstraction and provider implementations |
| [Responses usage](../../crates/hame-llm/src/responses.rs) | Cached input subtracted from ordinary input count |
| [Chat-completions usage](../../crates/hame-llm/src/chat_completions.rs) | Cached input normalized separately |
| [Completion checks](../../crates/hame-verify/src/redcheck.rs) | Code-oriented stopping checks and bounded nudges |
| [Evaluation runner](../../crates/hame-cli/src/eval.rs) | Hidden checks and resource metrics; measured agent duration excludes subsequent external checker |

## Refresh procedure

1. Re-query metadata and record full commits; keep the old manifest so results remain reproducible.
2. Re-read changed adapter contracts, platform support, licenses and model-accounting behavior.
3. Run conformance tests against pinned releases before changing dependencies.
4. Re-run performance comparisons when execution semantics or resource use change.
5. Preserve failed lookups and missing usage as explicit unknowns; do not replace them with assumptions.
