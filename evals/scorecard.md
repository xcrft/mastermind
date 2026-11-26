# Evaluation scorecard

This is the index of retained results behind Mastermind's claims. Each row names the report, the version and date it measured, and what it leaves unmeasured. A result holds only for the version it names, so a later release needs a new matched run before it reuses a claim. How to run the checks is in the [evaluation README](README.md).

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
| Without time, token or turn budgets, research instructions with graph tools used 25.78% fewer tokens and 14.60% more time than source tools alone. The yield gain per token is only bounded, between -43.86% and +196.42% | [Unbounded research](baselines/research-unbounded-20261008.json) | 3.2.1 binary, Codex 0.160.1, 2026-10-08 | A positive overall efficiency gain, independent review, held-out tasks |
| The shorter research instruction is rejected. It used 7.15% fewer tokens per accepted task, but p95 latency was 15.51% higher and the ambiguous-symbol task used 91.8% more tokens | [Instruction calibration](baselines/instruction-optimization-20261008.json) | 3.2.2, Codex 0.162.0-alpha.2, 2026-10-08 | Independent review, prompt refinement, personalization |
| The `bounded_readonly_v1` effort router stays experimental. Each arm has 7 of 8 tasks accepted after the complete-answer recheck | [Effort routing](baselines/effort-routing-20261008.md), [recheck](baselines/effort-quality-recheck-20261008.json) | `0db5dfe`, 2026-10-08 | Preserved quality on the full corpus, tokens per accepted task |
| Neither routed nor batched retrieval instructions are promoted. Routing kept 8 of 8 accepted with 6.1% more tokens, batching dropped to 7 of 8 | [Context routing](baselines/context-routing-20261010-public.json) | 3.2.4 at `6da6a73`, 2026-10-10 | Accepted-task efficiency, effect of batching |
| `mmcg repo-map` cut total producer tokens by 6.9% across 8 development questions in one run | [Repo-map](baselines/repo-map-20261012.json) | 3.2.4 at `f9c336c`, 2026-10-12 | General benefit, production default |
| No repo-map arm beat the baseline on Astra acceptance: bundle 14 of 16, map-only 12 of 16, baseline 14 of 16. The bundle v2 navigation instruction scored 10 of 16 and was not promoted | [Factorial](baselines/repo-map-factorial-20261013.json), [boundaries v2](baselines/repo-map-boundaries-v2-20261013.json) | 2026-10-13, 8 development tasks | Human reference labels, production improvement |
| Mandatory source quotes did not improve acceptance, 14 of 16 against 15 of 16, and raised output tokens by 56.59% | [Source quotes](baselines/source-quotes-20261014.json) | 3.2.4 at `f9c336c`, 2026-10-14 | Claim correctness of quoted answers |
| A stricter offline parser finds 227 valid source bindings in the quoted answers, against 84 before. Under parser v4, 9 snippets fall outside their declared ranges and 100 match only the raw fenced body, so they stay unverifiable | [Parser reaudit](baselines/producer-binding-reaudit-20261014.json), [framing](baselines/producer-binding-framing-20261014.json) | 2026-10-14, the same 32 answers | Claim correctness, any new model result |
| No contrast in the six development campaigns shows a detectable acceptance difference. Low effort used 34.2% fewer total tokens and 44.5% less producer run time than high effort, exact p = 0.008 | [Statistical reanalysis](baselines/statistical-reanalysis-20261015.md) | 2026-10-15, no new model calls | Effects outside these 8 tasks, acceptance non-inferiority |
| Eight tasks with eight repetitions per arm give 68.6% power, not 80%, for a 0.90 token ratio | [Resource power](baselines/resource-power-20261015-v2.json) | 2026-10-15, simulation only | Any model result |
| Source-witness concept review keeps all 574 useful identities, but strict qualification fails on 3 disagreements and 1 missing declaration | [Concept witness](baselines/concept-witness-20261016.md) | 2026-10-16 | Qualified production use |

## Harness and rejected candidates

None of these measures a product benefit.

| Claim | Report | Date | Not measured |
|---|---|---|---|
| Codex stage accounting passed 386 deterministic tests and detected 4 of 4 guard mutations | [Stage accounting](baselines/codex-stage-accounting-20261008-public.json) | 2026-10-08 | Semantic quality, model savings |
| The effort-routing harness passed 389 tests and detected 4 of 4 mutations | [Effort harness](baselines/effort-routing-harness-20261008.json) | 2026-10-08 | Output correctness |
| The returned-range replay accounts for all 16 planned instruction attempts | [Range replay](baselines/retrieval-replay-20261008.json) | 2026-10-08 | Safe removal of repeated reads |
| Native batch lookup returns the same query metadata as 3 single lookups | [Batched lookup](baselines/retrieval-implementation-20261008.json) | 2026-10-08 | Task quality, latency or token gain |
| The evidence-policy candidate is rejected. Neither tokens per accepted task nor median latency improved by 10% | [Evidence policy](baselines/evidence-frontier-20261008-public.json) | 2026-10-08 | Other tasks |
| The retrieval-route candidate is rejected. Token savings stayed under 10% and p50 latency got worse | [Retrieval route](baselines/route-selection-20261008-public.json) | 2026-10-08 | Other tasks |
| Source reuse stays off. It accepted 7 of 8 against 8 of 8 with full reads and used 20.6% more tokens per accepted task | [Source delivery](baselines/source-delivery-20261010-public.json) | 2026-10-10 | Token savings from reuse, independent review |
| The 3.2.1 product baseline compares `source` with `portable_mmcg` over 36 attempts on 4 tasks, scored against the full source key | [Product baseline](baselines/product-20261008-03.json) | 2026-10-08 | User acceptance |

## Not measured yet

| Feature | Measurement still needed |
|---|---|
| Prompt refinement | Raw and refined accepted tasks under one original key, with generation cost |
| Personal profile | Real-history labels, plus no-profile and shuffled-profile controls on the same tasks |
| Project and document context | Relevant, irrelevant and stale context on unseen tasks |
| Research and graph quality | Held-out tasks with independently reviewed complete answers |
| Whole workflow | Accepted coding outcomes, false completion and user corrections |
| Cost and throughput | Complete usage and accepted outcomes under matched budgets. Subscription billing stays unknown |
| Indexing | Mixed-language repositories and production storage |
| Behavioral role suites | A complete model-backed run on a released version |

Repetitions of the same tasks are not independent samples, and no qualified population or product-quality result exists yet.
