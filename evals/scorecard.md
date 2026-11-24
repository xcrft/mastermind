# Evaluation scorecard

Use this page to find which retained report supports a claim, which version
and date it measured, and what it leaves unmeasured. A result holds only for
the version it names. A later release needs a new matched run before it can
reuse a claim. Each report keeps its own counts, failures and limits.

Run instructions and comparison rules are in [evals/README.md](README.md).
Product objectives and open gaps are in [Product success measures](PRODUCT.md).
The full write-up of the 2026-10-07 and 2026-10-08 runs, with review findings,
per-task tables and superseded records, is in
[the 2026-10-09 record](baselines/scorecard-20261009.md).

## Deterministic checks

| Claim | Report | Version and date | Not measured |
|---|---|---|---|
| 372 of 372 Python harness and repository contract tests passed | [Source pagination](baselines/source-pagination-20261008.json) | 3.2.2 at `aca0ae5`, 2026-10-08 | Model quality, speed or user benefit |
| Bounded source reads resolved 71 of 71 earlier read failures, with no new failure in 674 recorded requests | Same report | 3.2.2 at `aca0ae5`, 2026-10-08 | Model speed or quality gain |
| Retained tests detect every selected production mutation: harness 10 of 10, disabled budget 3 of 3, source read 4 of 4, outcome range 2 of 2, document discovery 2 of 2, product report 3 of 3, profile delivery 4 of 4 | [Source read](baselines/source-pagination-20261008.json), [harness](baselines/harness-mutations-20261007.json), [citation root](baselines/harness-citation-mutation-20261007.json), [continuation](baselines/harness-continuation-mutation-20261007.json), [disabled budget](baselines/harness-unbounded-mutations-20261007.json), [outcome range](baselines/harness-yield-mutations-20261008.json), [document discovery](baselines/harness-source-scope-mutations-20261008.json), [product report](baselines/product-guard-mutations-20261008.json), [profile delivery](baselines/profile-regression-mutations-20261008.json) | Source of 2026-10-07 and 2026-10-08 | Mutation coverage of the code base |
| 91 of 91 native CLI and library tests passed in 15 targets | [Control report](baselines/control-main-322-20261008-public.json) | 3.2.2 at `aca0ae5`, 2026-10-08 | Semantic goal success, token and cost savings |
| The finite completion model has 65 states, 586 transitions and no publication violation, and each of 6 omitted guards is detected | Same report | 3.2.2 at `aca0ae5`, 2026-10-08 | Refinement of the Rust implementation, concurrent transitions |
| An unchanged index reuses its cache with median latency 93.22% lower than a forced full reparse | [Index benchmark](baselines/index-20261007.json), [method](../docs/benchmarks.md) | `86b46d4`, 2026-10-07, 1,000-file Rust fixture | Mixed-language repositories, gain between versions |
| Reindexing 100 changed files out of 1,000 has median latency 83.52% lower than a full reindex | Same report | `86b46d4`, 2026-10-07, same fixture | Mixed-language repositories, gain between versions |
| Style detector V2 finds 90.6% of labeled explicit statements, V1 finds 31.3%, both with 0 of 26 false positives | [Detector](baselines/persona-detector-20261007.json) | `86b46d4`, 2026-10-07, 58 synthetic examples | Accuracy on real user history, truth or usefulness of inferred rules |
| Local hook extraction has 100% precision and 91.18% recall on exact quotes | [Local extraction](baselines/persona-local-20261007.json) | 2026-10-07, 60 synthetic examples | Held-out cases, task benefit |

## Model-backed runs

| Claim | Report | Version and date | Not measured |
|---|---|---|---|
| With 180 s, 16,384 output tokens and 8 turns per attempt, 32 of 36 research attempts timed out and the graph arm satisfied 0 of 12 | [Bounded research](baselines/research-20261007.json) | 3.2.1 binary, Codex 0.160.1, 2026-10-07 | Benefit on unseen tasks, token or cost saving |
| Without time, token or turn budgets, research instructions with graph tools used 25.78% fewer tokens and 14.60% more time than source tools alone. The yield gain per token is only bounded, between −43.86% and +196.42% | [Unbounded research](baselines/research-unbounded-20261008.json) | 3.2.1 binary, Codex 0.160.1, 2026-10-08 | A positive overall efficiency gain, independent review, held-out tasks |
| The shorter research instruction is rejected. It used 7.15% fewer tokens per accepted task, but p95 latency was 15.51% higher and the ambiguous-symbol task used 91.8% more tokens | [Instruction calibration](baselines/instruction-optimization-20261008.json) | 3.2.2, Codex 0.162.0-alpha.2, 2026-10-08 | Independent review, prompt refinement, personalization |
| The `bounded_readonly_v1` effort router stays experimental. Each arm has 7 of 8 tasks accepted after the complete-answer recheck | [Effort routing](baselines/effort-routing-20261008.md), [recheck](baselines/effort-quality-recheck-20261008.md) | `0db5dfe`, 2026-10-08 | Preserved quality on the full corpus, tokens per accepted task |
| Neither routed nor batched retrieval instructions are promoted. Routing kept 8 of 8 accepted with 6.1% more tokens, batching dropped to 7 of 8 | [Context routing](baselines/context-routing-20261010.md) | 3.2.4 at `6da6a73`, 2026-10-10 | Accepted-task efficiency, causal effect of batching |
| `mmcg repo-map` cut total producer tokens by 6.9% and uncached input by 12.0% across 8 development questions in one run | [Repo-map](baselines/repo-map-20261012.md) | 3.2.4 at `f9c336c`, 2026-10-12 | General benefit, production default |
| No repo-map arm beat the baseline on Astra acceptance: bundle 14 of 16, map-only 12 of 16, baseline 14 of 16. The bundle v2 navigation instruction scored 10 of 16 and was not promoted | [Factorial](baselines/repo-map-factorial-20261013.md), [boundaries v2](baselines/repo-map-boundaries-v2-20261013.md) | 2026-10-13, 8 development tasks | Human reference labels, production improvement |
| Mandatory source quotes did not improve acceptance, 14 of 16 against 15 of 16, and raised output tokens by 56.59% | [Source quotes](baselines/source-quotes-20261014.md) | 3.2.4 at `f9c336c`, 2026-10-14 | Claim correctness of quoted answers |
| A stricter offline parser finds 227 valid source bindings in the quoted answers, against 84 before, and 109 snippets outside their declared ranges | [Parser reaudit](baselines/producer-binding-reaudit-20261014.md), [framing](baselines/producer-binding-framing-20261014.md) | 2026-10-14, the same 32 answers | Claim correctness, any new model result |
| No contrast in the six development campaigns shows a detectable acceptance difference. Low effort used 34.2% fewer total tokens and 44.5% less producer run time than high effort, exact p = 0.008 | [Statistical reanalysis](baselines/statistical-reanalysis-20261015.md) | 2026-10-15, no new model calls | Effects outside these 8 tasks, acceptance non-inferiority |
| Eight tasks with eight repetitions per arm give 68.6% power, not 80%, for a 0.90 token ratio | [Resource power](baselines/resource-power-20261015.md) | 2026-10-15, simulation only | Any model result |
| Source-witness concept review keeps all 574 useful identities, but strict qualification fails on 3 disagreements and 1 missing declaration | [Concept witness](baselines/concept-witness-20261016.md) | 2026-10-16 | Qualified production use |

## Not measured yet

| Feature or outcome | Retained evidence | Measurement still needed |
|---|---|---|
| Prompt refinement | Production protocol checks and static condition controls | Raw and refined accepted tasks under one original key, generation cost and native delivery |
| Personal profile | Attribution, store and replay regressions, synthetic extraction labels | Independent real-history labels, contradictions, applicability, no-profile and shuffled-profile controls |
| Project and document context | Freshness, audience, scope and coverage checks | Relevant, irrelevant and stale context contrasts on unseen tasks |
| Research and graph quality | Four-task public calibration runs and eight-task development campaigns | Held-out tasks, realistic repository discovery and independently reviewed complete answers |
| Whole workflow | Finite guards and selected CLI paths | Accepted coding outcomes, false completion, corrections and bounded failure and recovery comparisons |
| Cost and throughput | All-attempt telemetry and useful-outcome formulas | Complete usage, accepted outcomes and matched budgets. Subscription billing stays unknown |
| Representative indexing | Synthetic Rust call-chain fixture | Mixed-language repositories, imports, generated files and production storage and load |
| Behavioral role suites | Loader and fixture tests | A complete model-backed run on a released version |

For a comparable resource `C` and known successful outcomes `S`, resource per
success is `C / S` and useful-work yield is `S / C`. Count every planned
attempt and keep missing values unknown. Relative yield gain is undefined when
the baseline yield is zero. Repetitions of the same tasks are not independent
samples. No qualified population or causal product-quality result exists.
