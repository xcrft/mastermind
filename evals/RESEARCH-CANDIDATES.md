# Research candidates: open techniques for Mastermind (2026-10-12)

External survey of open-source algorithms, tools and agent practices applicable to
Mastermind's four measured axes (latency, input tokens, output tokens, acceptance).
This complements [OPTIMIZATION.md](OPTIMIZATION.md): it does not re-survey methods
already implemented or proposed there (batched lookup, bounded reads, returned-range
ledger, effort routing, Adaptive-RAG-style router, submodular selection, GEPA/MIPROv2,
bandits, conformal risk, VOI stopping, DiffAdapt, ledger reuse). Every external claim
cites its primary source inline. Claims marked **[inference]** are ours, not the source's.
Constraints honored: subscription CLIs (no hidden states, no logit access), Rust core,
local-first, cheap deterministic interventions first.

Verification note: the often-cited "speculative tool calling, arXiv 2510.13625" is
**not** that paper — 2510.13625 is a humanoid-robot YOLO vision paper
(https://arxiv.org/abs/2510.13625). Verified alternatives are cited under Speed.

## Axis 1: Latency / speed

| Name | What | Primary source | Maturity / license | Axis impact | Mastermind surface | Measurement sketch | Risks |
|---|---|---|---|---|---|---|---|
| Speculative tool calling (small-model prefetch) | A 3B model predicts upcoming tool calls and pre-executes them while the main LLM still reasons; 1.6–2.2× speedup for agents with Qwen2.5-3B / Llama-3.2-3B | [arXiv:2605.13360](https://arxiv.org/abs/2605.13360); related [arXiv:2512.15834](https://arxiv.org/abs/2512.15834) | Preprints (2026-05, 2025-12); research code unclear | Big agent-side latency cut; needs an extra local model | None directly — needs model access we lack; motivates a **deterministic** mmcg-side variant **[inference]**: after `mmcg_search` returns symbols in file F, pre-warm the read/outline cache for F (CPU-only, no model) | 2-arm condition on tool intervals (`diagnostics.adapter.tool_timeline`): prefetch on vs off; primary metric = interval-union latency at equal acceptance | Prefetching is guessing; wasted CPU is free but adds no token saving. **Measured:** the retained-trial tool-interval union is 0.197% of wall time (`cache-audit-20261012`, local report, not retained in the repository), so direct prefetch headroom in this sample is small; the remaining latency lever is fewer model rounds, not faster tool serving |
| SPORK (self-speculative forking) | Forkes the model's own decoding to predict the next tool call early and overlap execution with chain-of-thought decoding; training-free | [arXiv:2607.03333](https://arxiv.org/abs/2607.03333); code https://github.com/baihuajun24/spork | Preprint (2026-07), open code | Overlaps tool latency with reasoning | Requires serving-side control of the model — not available via subscription CLIs | Not measurable under current harness; document as future runtime contract | Same as DiffAdapt-class methods in OPTIMIZATION.md: runtime contract missing |
| Parallel tool calls | One assistant turn may contain several `tool_use` blocks; results returned together per `tool_use_id`; MCP `tools/call` is JSON-RPC so calls can be issued concurrently | [Anthropic tool-use overview](https://platform.claude.com/docs/en/docs/agents-and-tools/tool-use/overview); [MCP tools spec](https://modelcontextprotocol.io/specification/2025-06-18/server/tools) | Stable, vendor-supported | Fewer round trips → lower p50/p95 latency; no token change expected | Instruction module: "issue independent lookups in one turn" (mmcg broker already serializes? verify) **[inference]** | Instruction A/B (module on/off), same tools/model; compare `mcp_calls` clustering, tool-interval union, p95 | Client/CLI must actually emit parallel calls. **Measured:** retained Codex streams already show multiple outstanding MCP calls in 32/32 attempts (`cache-audit-20261012`, local report, not retained in the repository), so an instruction arm has little observable headroom in this sample |
| vLLM automatic prefix caching | Reuses KV blocks across requests sharing a prefix; speeds prefill only, not decode | [vLLM docs](https://docs.vllm.ai/en/latest/features/automatic_prefix_caching.html) | Production, Apache-2.0 | Only if the judge is ever self-hosted | Judge pipeline (if self-hosted judge model) | Judge replay benchmark: judge latency/cost on same review set with APC on/off | No effect while judge is a pinned Claude CLI; local-first constraint |
| Tantivy full-text index | Lucene-like Rust search: BM25, multithreaded indexing, <10 ms startup; self-benchmark ~2× Lucene search latency | [GitHub](https://github.com/quickwit-inc/tantivy) | 16k+ stars, active, MIT | Local search latency only | `store.rs`/`queries.rs` — replaces/augments SQLite FTS5 (verified: `symbol_concepts_fts` uses fts5) | Only if native search shows in tool intervals; then microbench FTS5 vs tantivy on frozen corpora | FTS5 already sufficient at repo scale **[inference]**; second index store duplicates truth (violates single-store contract) |

## Axis 2: Input / context tokens

| Name | What | Primary source | Maturity / license | Axis impact | Mastermind surface | Measurement sketch | Risks |
|---|---|---|---|---|---|---|---|
| Aider repo map (tree-sitter + PageRank) | Sends a ~1k-token map of files + key definitions, ranked by a graph algorithm over file dependencies; code uses `nx.pagerank` over a MultiDiGraph of tree-sitter refs | [Docs](https://aider.chat/docs/repomap.html); [repomap.py](https://github.com/Aider-AI/aider/blob/main/aider/repomap.py) | Mature, Apache-2.0 | Replaces early full-file reads with a tiny skeleton; fewer input tokens | New `mmcg` capability or instruction: budgeted repo-map header in the first reply; ranking reuse of existing graph edges | Condition `tools: mmcg` + map-on vs map-off; uncached input tokens + tool rounds; acceptance gate via blinded review | Map ≠ evidence: answers citing the map need real reads; ranking may hide the needed symbol |
| Agentless localization-first | Fixed pipeline localize → repair → validate, no agent loop; 32% SWE-bench Lite at ~$0.70/task, cheaper than agentic baselines | [arXiv:2407.01489](https://arxiv.org/abs/2407.01489); code [OpenAutoCoder/Agentless](https://github.com/OpenAutoCoder/Agentless) (MIT) | Paper + code; repo lightly maintained | Structurally fewer tool rounds/tokens than open-ended exploration | Instruction module: staged plan (locate via graph → read narrow ranges → verify) — a *discipline* port, not the pipeline itself **[inference]** | Instruction A/B on staged vs current research skill; rounds, uncached tokens, acceptance | Their tasks are read-only research, not patching; savings claim from a different task class |
| RepoCoder iterative retrieval-generation | Draft answer → use draft as retrieval query → refine; beats non-iterative retrieval and in-file baselines by >10% on repo-level completion | [arXiv:2303.12570](https://arxiv.org/abs/2303.12570); code [microsoft/CodeT](https://github.com/microsoft/CodeT/tree/main/RepoCoder) | EMNLP 2023, code available | Better second-round retrieval → fewer wasted reads | Instruction nudge: "re-query with the symbols found in your partial answer" before finalizing | Instruction A/B; count retrieval rounds with recall (unique relevant ranges) from ledger | More rounds can raise tokens; benefit shown for completion, not Q&A research |
| Prompt-caching discipline (Anthropic) | Cache = exact prefix match at `cache_control` breakpoints; order tools→system→messages; ≥512-token minimums; 5 m/1 h TTL; writes 1.25×/2×, reads 0.1×; changing tool defs or thinking budget invalidates cache; 20-block backward hit walk | [Anthropic caching docs](https://platform.claude.com/docs/en/docs/build-with-claude/prompt-caching) | Official docs, production | Converts uncached input to 0.1× reads — measured axis | Broker/tool schema ordering: freeze tool order + instruction bytes across a trial (already per-condition); verify CLI keeps stable prefixes | Reanalysis of existing runs: correlation of cache-read share with tool-result ordering in stream; then an ordering-fixed arm | Mostly client-controlled; subscription CLI hides breakpoint placement |
| Prompt caching (OpenAI side) | Caching behavior **differs by model generation** (implicit endpoints vs explicit breakpoints); the survey's earlier uniform "automatic caching at ≥1,024 visible tokens, reads ~0.1×" claim is too broad — corrected against the live guide on 2026-10-12 in `cache-audit-20261012` (local report, not retained in the repository); a shared prefix alone need not be reusable, and API pricing does not establish subscription billing or historical CLI behavior | [OpenAI caching guide](https://developers.openai.com/api/docs/guides/prompt-caching) | Official docs | Converts uncached input to cheaper reads — measured axis | Same: stable tool catalog per trial (broker already pins catalog) | **Audit outcome:** retained Codex trials report aggregate usage only (one `turn.completed` per attempt) with cache writes recorded as 0 in 32/32, so an ordering effect on misses cannot be identified from these records; an ordering A/B can still measure aggregate resource and acceptance outcomes, with the mechanism unknown | Provider-side TTL/routing outside our control; request-level usage and breakpoint identity not exposed by the subscription CLI |
| Cross-encoder reranking (bge-reranker) | Local cross-encoder reorders BM25/dense top-k; more accurate than bi-encoder retrieval | [FlagEmbedding](https://github.com/FlagOpen/FlagEmbedding) | MIT, active (12k+ stars) | Higher evidence recall per delivered token (fewer irrelevant ranges read) | `queries.rs`: rerank `mmcg_search` top-N on CPU before delivery | 2-arm tool comparison (rerank on/off, same `top`); unique relevant ranges per token, then model A/B | Adds a bundled model → violates "no extra model calls preferred" spirit; adds latency; local-first kept (ONNX/CPU) **[inference]** |
| LLMLingua-2 prompt compression | XLM-RoBERTa token-classifier drops tokens; 2–5× compression, 3–6× faster than LLMLingua, 1.6–2.9× end-to-end speedup | [arXiv:2403.12968](https://arxiv.org/abs/2403.12968); code https://aka.ms/LLMLingua-2 | ACL 2024 Findings; Microsoft code | 2–5× fewer input tokens on compressible text | None recommended for source evidence | — | Compressing source/tool results breaks exact-quote review bindings and source-preservation rules; conflicts with AGENTS.md "preserve source bindings" |
| Sub-agent context isolation | Subagent runs in a fresh window; only its summary returns to the main thread; counts toward same usage limits | [Claude Code subagents](https://code.claude.com/docs/en/sub-agents) | Official feature | Keeps flood-prone sweeps (outlines, logs) out of main context | Instruction module ("delegate exhaustive enumeration to a subagent") — needs a harness arm exposing the Task tool | New condition permitting the subagent tool; main-thread input tokens vs total usage; acceptance gate | Harness currently exposes only declared tools (adapter change); savings depend on delegation actually happening |
| MemGPT/Letta memory paging | OS-style paging between context window and external memory tiers | [arXiv:2310.08560](https://arxiv.org/abs/2310.08560); https://memgpt.ai | Paper 2023; Letta product | Long-task context reuse | Not applicable to short benchmark trials; product-side idea only | — | Requires framework ownership of the loop; conflicts with frozen adapter contract |
| Auto-compact / `/compact` costs | Auto-compact summarizes near limits; `/compact` itself reads the whole conversation (expensive) and is an expected cache miss; `/clear` is free | [Claude Code costs](https://code.claude.com/docs/en/costs) | Official docs | Avoidable input-token spikes | Instruction: prefer `/clear`-style fresh scoped trials over compaction mid-task **[inference]** | Detect compaction events in runs; compare uncached-token spikes | Summarized history loses tool-result bindings our review needs (exact quotes) **[inference]** |

## Axis 3: Output tokens

| Name | What | Primary source | Maturity / license | Axis impact | Mastermind surface | Measurement sketch | Risks |
|---|---|---|---|---|---|---|---|
| Reasoning-effort tiers (OpenAI) | `reasoning.effort` none…max; `low` suited to tool use / execution-oriented coding; mid-conversation change via `configuration_update` preserves cache prefixes (GPT-6 family) | [OpenAI reasoning guide](https://developers.openai.com/api/docs/guides/reasoning) | Official | Fewer reasoning tokens (subset of output) on bounded tasks | Extends implemented `bounded_readonly_v1` with a `low` arm for the most bounded class | 2-arm effort comparison (`low` vs `high`) under the frozen condition contract; output tokens + acceptance gate | Already partially covered by OPTIMIZATION.md effort routing — only the `low` tier and cache-preserving switch are new |
| Stable thinking budget (Anthropic) | `budget_tokens` ≥1,024, is a target not a cap; **changing it invalidates message cache breakpoints**; keep budget stable per conversation; effort + adaptive thinking on 4.5+ | [Extended thinking docs](https://platform.claude.com/docs/en/docs/build-with-claude/extended-thinking) | Official | Avoids cache-rebuild input spikes; indirectly caps reasoning output | Judge + Claude adapter: pin one effort/budget per trial (already the spirit; make it an explicit check) | Regression check on retained runs: cache-write spikes after effort changes | CLI maps `--effort`; budget surface may be hidden |
| Structured outputs (strict JSON schema) | Guarantees schema-conformant output; first request per schema has extra compilation latency, later ones cached | [OpenAI structured outputs](https://developers.openai.com/api/docs/guides/structured-outputs) | Production | Judge retries ↓ (fewer malformed-JSON repeats) | Judge pipeline schema enforcement + cheap local validation | Judge-side replay: retry count / parse failures before vs after strict mode | Claude CLI path offers no strict schema; schema limits (unsupported JSON Schema features) |
| Outlines / xgrammar constrained decoding | Logit masking from regex/JSON-schema/CFG; xgrammar compiles grammars to token masks with near-zero JSON overhead, integrated in vLLM/SGLang/TensorRT-LLM | [Outlines](https://github.com/dottxt-ai/outlines) (Apache-2.0); [xgrammar](https://github.com/mlc-ai/xgrammar) (Apache-2.0) | Both active, production users | Deterministic judge verdicts, zero retries — if judge is self-hosted | Self-hosted judge only; today no logit access via CLI | Same replay benchmark on a local judge (e.g., Qwen + xgrammar) vs pinned Claude judge | Introducing a self-hosted judge changes judge identity — keep as fallback, not default |
| GPTCache semantic caching | Caches LLM responses by exact/similarity match; MIT | [GitHub](https://github.com/zilliztech/GPTCache) | 8.2k stars; maintainers no longer add new APIs/models | Skip repeat identical LLM calls | Not recommended: ledger already avoids repeated tool delivery; approximate similarity reuse conflicts with freshness/identity reuse rules in OPTIMIZATION.md | — | Semantic (approx) matches can serve wrong evidence; maintenance slowing |

## Axis 4: Task acceptance quality

| Name | What | Primary source | Maturity / license | Axis impact | Mastermind surface | Measurement sketch | Risks |
|---|---|---|---|---|---|---|---|
| Localization-first discipline (Agentless port) | Cheaper structured localize→validate beats long agentic loops on their benchmark | [arXiv:2407.01489](https://arxiv.org/abs/2407.01489) | See Axis 2 | Higher acceptance at same/lower tokens | Same instruction module as Axis 2 | Same A/B, gated on acceptance not tokens | Port across task class unproven |
| RepoCoder-style second-pass retrieval | Iteration raises retrieval quality >10% over one-shot on their benchmark | [arXiv:2303.12570](https://arxiv.org/abs/2303.12570) | See Axis 2 | Better evidence coverage | Same instruction nudge | Coverage (unique relevant ranges) + acceptance | — |
| Reranked retrieval | Cross-encoder rerank raises top-k precision | [FlagEmbedding](https://github.com/FlagOpen/FlagEmbedding) | See Axis 2 | Acceptance via better first reads | Same tool arm | See Axis 2 | — |
| SWE-bench Verified methodology | Fail-to-pass + pass-to-pass test gating; human-validated 500-instance subset | [arXiv:2310.06770](https://arxiv.org/abs/2310.06770); https://www.swebench.com; [GitHub](https://github.com/SWE-bench/SWE-bench) | ICLR 2024, standard | Measurement reference only | Regression-gated fix tasks in future corpora (their rubrics already request failing-check evidence) | Steal: every "fix" task ships a test that fails before / passes after | Read-only research corpus ≠ patch corpus |
| Aider polyglot leaderboard | 225 Exercism tasks × 6 languages; percent correct over two attempts; records cost/tokens/edit-format per run | [Leaderboard docs](https://aider.chat/docs/leaderboards/); repo [Aider-AI/aider](https://github.com/Aider-AI/aider) | Mature, open runs | Measurement reference: multi-language task battery + per-run token/cost disclosure | Corpus expansion pattern for language coverage (tree-sitter extractors × languages) | Add per-language calibration cases in the same paired design | Edit-format variance is an aider-specific confound we don't share |
| terminal-bench | Terminal tasks in Docker; end-state test scripts; oracle solutions | [GitHub](https://github.com/laude-institute/terminal-bench) | Apache-2.0, ICLR 2026 | Measurement reference: end-state (not transcript) scoring | Reinforces their acceptance = original request fulfilled, not process shape | — | — |
| OpenHands condenser pattern | Event-stream condensation: drop events, insert summaries to fit context | SDK paper via [OpenHands](https://github.com/All-Hands-AI/OpenHands); arXiv SDK paper 2025 | Active | Product idea for long sessions | Not for benchmark trials | — | Same binding-loss risk as compaction |
| FrugalGPT cascade | Learned LLM cascade matches best model at up to 98% cost cut | [arXiv:2305.05176](https://arxiv.org/abs/2305.05176) | Paper 2023 | Not applicable: one pinned model per subscription condition | — | — | Conflicts with same-model A/B; supersedes nothing |
| RouteLLM routing | Preference-data-trained router between strong/weak models, >2× cost cut | [arXiv:2406.18665](https://arxiv.org/abs/2406.18665) | Paper + open code | Not applicable without model-call control | Effort routing (already implemented) is the available analog under subscription CLIs | — | Same |

## Shortlist: cheapest-to-measure first

Ranked by (implementation cost × measurement cost × expected effect), all compatible
with the frozen condition contract (same source/key/model, paired tasks, blinded
review, acceptance gate + token/latency deltas). Updated 2026-10-12 after the
retained-run cache audit (`cache-audit-20261012`, local report, not retained in the repository).

**Executed: cache-order audit (former #1).** Retained trials already reuse cache
heavily: 83.99% of 11.3M reported input tokens are cache reads (range 73.4–89.4%
per attempt), but usage events are aggregate (`turn.completed`) with cache writes
recorded as zero in 32/32, so a tool-result ordering effect cannot be identified
from these records. Decision: cache ordering stays an **unqualified hypothesis**;
a future ordering comparison needs request-level usage and prefix/breakpoint
identity, or must fall back to aggregate outcome A/B with unknown mechanism. The
audit also demoted two latency candidates: prefetch headroom is 0.197% of wall
time (tool-interval union), and the client already issues overlapping MCP calls
in 32/32 attempts.

**Executed since 2026-10-12 (all on the eight single-repository development tasks):**

- **Repo-map bundle** (3282e4f): −6.9% total tokens, −12.0% uncached; provisional.
- **Factorial decomposition** ([repo-map-factorial-20261013](baselines/repo-map-factorial-20261013.md)): baseline 14/16, map-only 12/16 (−9.4% total), instructions-only 13/16, bundle 14/16 (−10.1% uncached, −8.5% total). CPU budget sweep collapsed: 1,600/4,096/16,384 caps produce identical payloads; 512 is the only other distinct point.
- **Graph ranking variant** (`repo-map-graph-20261013`, local report, not retained in the repository): personalized-PageRank over lexical seeds — **rejected** (+9.5% total tokens, +2.4% uncached, one paired satisfied→unsatisfied loss). Lexical stays the default; graph is opt-in.
- **Boundary instruction v2** ([repo-map-boundaries-v2-20261013](baselines/repo-map-boundaries-v2-20261013.md)): **rejected** (10/16 vs 14/16, savings not preserved, targeted checks failed); original 171-byte instruction restored.
- **`low`-effort arm** (`repo-map-effort-20261013`, local report, not retained in the repository): the largest win so far — 13/16 vs 11/16 satisfied, output −52.9%, total −36.0%, uncached −18.1%, tools −24.0%; one paired loss on reference-removal. Production routing shipped in a5c3b71 (`mmcg effort`, conservative `low` classes only).
- **Blind Astra re-review** (`repo-map-components-20261012`, local report, not retained in the repository): Sol/Astra agree on 12/14; the first retained human outcome disagrees with Astra. Same nominal baseline condition scored 11/16, 13/16 and 14/16 across campaigns — between-campaign acceptance noise is ±3/16.
- **Effort routing to ordinary handoffs + release 3.3.0** (873e465, 5b01e79): routing extended beyond the special low-classes into the shipped workflow templates and docs.
- **Judge source-quote binding audit** (c30c519, 77970dc): judge instruction now requests structured `source_quotes`; the runner audits receipts automatically (`benchmark/evidence_binding.py`), recording valid/invalid anchors and audit digests. First mechanical evidence on judge reliability: 3 invalid quotes found inside satisfied judgments. A/B of **mandatory producer snippets was rejected** ([source-quotes-20261014](baselines/source-quotes-20261014.md)): 15/16 vs 14/16 with one paired loss, output tokens +56.59%, duration +43.89% — forcing snippets into every answer does not pay; the mechanical audit stays.

Next steps after these results:

1. **Exact-profile judge calibration and human reference.** Run `judge_calibration`
   seeded defects against the Astra reviewer profile actually used by these gates
   (the components report states the older Sol calibration does not qualify it),
   then adjudicate human reference labels for retained answers, starting with the
   observed disagreements. Until then every ±1–3/16 gate decision rides on
   unquantified judge noise.
2. **Parser-gap fixes, then producer-side binding as a completion check.** The
   audit found the frozen producer parser misses 258 indented fenced blocks and
   55 filename-only references resolvable through the inventory. Fixing the parser
   is deterministic and extends mechanical binding coverage without model cost;
   after that, producer citation validation can become a `verify.run`-style check
   instead of forcing snippets into answers (the rejected variant).
3. **Multi-repository held-out qualification corpus.** All experiments run eight
   tuned tasks on one repository; formal qualification needs a preregistered corpus
   from at least two repositories with design-specific simulated thresholds
   (`qualify_calibration`). Include a power check: given ±3/16 between-campaign
   noise on identical arms, simulate the repetitions needed to resolve a −10%
   token effect with acceptance preserved.
4. **The documentation-concepts / concept task family.** All three negatives in the
   source-quotes run and prior concept failures concentrate in this family. Route
   concept-type questions to the concept FTS entry (adaptive entry routing) or
   improve the extraction contract; measure on that family first.
5. **Map × effort interaction.** The low-effort win was measured with the lexical
   map; the bundle token effect with high effort. One qualification-corpus arm
   with bundle + `low` (the production router's `low` classes) tests whether the
   savings compose. Also extract latency from the retained effort-run streams —
   the report has no time column, and the reanalysis is free.

Deferred (need runtime we don't have): speculative tool calling (2605.13360), SPORK,
RouteLLM/FrugalGPT cascades, constrained-decoding judge, LLMLingua-2 (rule conflict),
GPTCache (rule conflict), MemGPT paging.

Cross-check against OPTIMIZATION.md: nothing here supersedes its implemented or
proposed items; the caching-order audit and `low`-effort tier *extend* existing
effort-routing work; LLMLingua-2 and GPTCache were reviewed and rejected on
source-preservation grounds (AGENTS.md), which OPTIMIZATION.md's ledger-reuse rules
already anticipate.
