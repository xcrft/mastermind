# Evaluation scorecard

Use this file to locate retained results and decide which feature claim a
measurement supports. Run instructions and comparison rules are in
[evals/README.md](README.md).

Product objectives and current gaps are in [Product success measures](PRODUCT.md).
[The current instruction calibration](baselines/instruction-optimization-20261008.json)
records sixteen completed attempts with original-request assessments. The shorter
candidate is rejected. [The earlier product baseline](baselines/product-20261008-03.json)
preserves its old full-key outcomes and unmeasured user acceptance.

Main 3.2.2 now has a separate
[91-check native report](baselines/control-main-322-20261008-public.json), including
four profile-budget CLI tests and twelve library delivery regressions. The
36 model attempts below retain their 3.2.1 binary and source identities.

[Four profile-delivery production mutations](baselines/profile-regression-mutations-20261008.json)
were detected by retained tests; their source bytes were restored. This protects
selected 3.2.2 fixes and does not measure model behavior or overall coverage.

## Current deterministic checks

| Check | Result | Evidence | Boundary |
|---|---|---|---|
| Python harness and repository contracts | 372/372 passed | [Report](baselines/source-pagination-20261008.json) | Request acceptance, source pagination, legacy compatibility, latency, quality gates and process/source contracts; deterministic tests make no model calls |
| Source-read pagination | 71/71 prior read failures resolved, 0 new failures in 674 recorded requests | Same report | Complete ranges reconstructed from explicit bounded chunks; no model-speed or quality gain inferred |
| Source-read guard sensitivity | 4/4 selected production mutations detected | Same report | All-or-error response, hidden partial, repeated boundary and strict EOF |
| Codex continuation smoke | 390/390 requested lines read in two replies | Same report | One subscription-backed transport check, separate from deterministic tests and product qualification |
| Harness guard sensitivity | 10/10 selected production mutations detected | [8 guard mutations](baselines/harness-mutations-20261007.json), [citation root](baselines/harness-citation-mutation-20261007.json), [continuation guard](baselines/harness-continuation-mutation-20261007.json) | Cache accounting, runtime freeze, unknowns, failures, freshness, export identity, cleanup, citation directory and guarded continuation; not mutation coverage |
| Disabled-budget guard sensitivity | 3/3 selected production mutations detected | [Report](baselines/harness-unbounded-mutations-20261007.json) | Null settings, hidden process deadline and unwanted CLI output cap; private mutated copies, no model calls |
| Outcome-range arithmetic | 2/2 selected mutations detected | [Report](baselines/harness-yield-mutations-20261008.json) | Lower/upper yield pairing; arithmetic ranges, not confidence intervals |
| Documentation discovery | 2/2 selected mutations detected | [Report](baselines/harness-source-scope-mutations-20261008.json) | Local evidence excluded; an ignored ancestor cannot hide checked source |
| Product-report guard sensitivity | 3/3 selected mutations detected | [Report](baselines/product-guard-mutations-20261008.json) | Acceptance consistency, quality gate and observed latency span |
| Profile-delivery guard sensitivity | 4/4 selected production mutations detected | [Report](baselines/profile-regression-mutations-20261008.json) | Whole-list removal, fixed twelve-card cap, missing ranking and ignored init budget; source restored |
| Native CLI and library selection | 91/91 passed in 15 targets | [Report](baselines/control-main-322-20261008-public.json) | Main 3.2.2 production paths with fixture native clients/processors, including 16 profile-delivery regressions |
| Lens | 1 aggregate suite passed | Same report | DOM/static suite, no browser or model-use proof |
| Completion model | 65 states, 586 transitions, 0 publication violations | Same report | Six modeled obligations; implementation refinement remains a separate requirement |
| Guard sensitivity and recovery | 6/6 omitted guards detected; 64/64 open states can complete within 7 successful actions | Same report | Stable inputs and successful producers, no unconditional repair/convergence claim |
| Source identity | 187 measured files unchanged during control evaluation | Same report | Clean native checkout at `aca0ae5`; local Python harness bytes separately included in the source manifest |
| Repository validator | 40 artifacts, 0 errors, 0 warnings | `python3 scripts/validate.py` | Structural checks, not semantic quality |

The stable 3.2.2 control evaluation took 302.4 seconds including builds and fixtures.
Its `semantic_goal_success`, `token_savings` and `cost_savings` are null. Test
counts do not establish coverage percentages, zero hallucinations or user benefit.

## Measured mechanism effects

| Feature | Same-corpus comparison | Observed effect | Evidence and limit |
|---|---|---|---|
| Unchanged index cache | Cached vs forced full reparse | Median paired latency saving 93.22% | [3 raw runs](baselines/index-20261007.json), [method](../docs/benchmarks.md); 1,000 Rust call-chain files, fixed phase order |
| Incremental index | 100 changed files vs forcing all 1,000 files | Median paired latency saving 83.52% | Same report; 15 phases, no failed files, symbol/call/reference equivalence and fixture call checks |
| Explicit style detector | V1 vs V2 on 58 synthetic examples | Recall 31.25% → 90.625%, 0/26 false positives for both | [Report](baselines/persona-detector-20261007.json); author-written labels, not user-history accuracy |
| Local hook extraction | Exact complete quotes on 60 synthetic examples | 31 TP, 0 FP, 3 FN; precision 100%, recall 91.18% | [Report](baselines/persona-local-20261007.json); no held-out cases, no task-benefit pairs |

The indexing fixture differs from the older constant-only corpus. These are
within-current-implementation counterfactuals, not version-to-version gains.
Detector recall measures finding labeled statements; it does not measure the
truth, durability or usefulness of inferred personal rules.

## Instruction calibration on 3.2.2

Four public tasks, two arms and two counterbalanced repeats completed all sixteen
attempts with complete usage/timing. Both arms use graph tools and the same
source/key/runtime. Requested model/effort: `gpt-6.1-sol / max`; CLI:
0.162.0-alpha.2. Time, output-token and turn caps are null.

| Measure | Current instruction | Short candidate | Decision |
|---|---|---|---|
| Maintenance-declared request acceptance | 8/8 | 8/8 | No observed quality loss; independent review remains unmeasured |
| Tokens per accepted task, including cache | 288,838 | 268,189 | 7.15% lower, below 10% objective |
| Total latency p50 | 278.32 s | 273.03 s | 1.90% faster, below 10% objective |
| Total latency p95 | 317.26 s | 366.48 s | 15.51% slower |
| All-attempt total time | 2,239.66 s | 2,246.11 s | 0.29% slower |
| Ambiguous-symbol task token use | 382,358 | 733,435 | 91.8% higher at essentially unchanged average latency |

The candidate is retained as an experiment and is not installed as the default
research skill. [Bound report](baselines/instruction-optimization-20261008.json).
These results do not establish prompt-refiner, personalization, applied-fix or
population benefit. The pagination code change has separate evidence above;
its model resource benefit has not been measured.

## Completed research with a time budget

The retained bounded experiment uses Codex 0.160.1, the existing ChatGPT subscription,
requested model `gpt-6.1-sol`, reasoning effort `max`, a frozen native binary and
private v1 adapter bundles. It ran four current-source public calibration tasks,
three arms and three repetitions: 36 attempts. The common per-attempt limits are
180 seconds, 16,384 reported output tokens and 8 CLI conversation turns. Tool
calls and inference rounds are different from those turns.

| Contrast | What it tests |
|---|---|
| `source` vs `portable` | Added research-skill instructions |
| `portable` vs `portable_mmcg` | Added graph tools under the same instructions |
| `source` vs `portable_mmcg` | Combined static research instructions and graph tools |

[research-20261007.json](baselines/research-20261007.json) retains every attempt,
three contrasts, source/key/result hashes and all-attempt resource accounting.
All 36 planned attempts ran: 32 timeouts, three complete answers and one request
for an unavailable tool. The failed tool request was retained; input/runtime
rechecks allowed the final three planned attempts to continue without retrying it.

| Arm | Planned | Complete answers | Timeouts | Tool violation | Outcome from maintenance review | Total run time |
|---|---:|---:|---:|---:|---|---:|
| Source tools | 12 | 1 | 10 | 1 | 0–1 satisfied; one uncertain key/outcome judgment | 32.98 min |
| Added research instructions | 12 | 2 | 10 | 0 | 1 satisfied, one with broken citation links | 35.17 min |
| Instructions + graph | 12 | 0 | 12 | 0 | 0 satisfied | 35.92 min |

The graph arm made 59 graph requests. It used the feature, but did not complete
a task within this budget. The instruction-vs-source success delta is bounded
between 0 and +8.33 percentage points, without a point estimate. Graph-vs-instruction
is −8.33 points under the maintenance-agent judgments. These results do not
establish a benefit on unseen tasks or a useful token/cost saving.

| Limitation or defect | Treatment |
|---|---|
| Review | Source inspection by the implementation agent, not independent human evaluation |
| Uncertain baseline answer | A broader ordinary-call clause in the frozen key is not explicit; outcome stays unknown. Public key now accepts any supported function-value form requested by the task |
| v1 absolute links | One answer targets the nonexistent `client` tree; current v2 uses the frozen source directory and passed a rollback test |
| Unavailable tool | Codex requested `list_mcp_resources`; recorded as a failed boundary violation |
| Runtime provenance | Private v1 bundles stayed frozen; resumed coordinator used the current cleanup/continuation code |
| Source key | Rust precision pointer corrected publicly; the archived run retains its original key and unchanged criteria |
| Telemetry | Only three complete attempts have token usage; other token totals and all billing costs remain unknown |
| Identity | Requested model pinned, actual served model not reported |
| Corpus | Four public curated source projections, no held-out tasks or whole-repository discovery |

The refreshed keys require independent review. This experiment does not measure
native prompt refinement, profile mining or coding-task benefit. The nullable-budget
v3 adapter has its own experiment below. Keep this bounded baseline intact.

## Research without an experiment budget

The separate v3 experiment completed all 36 attempts: four public tasks, three
arms and three balanced repetitions. It uses the same requested model/effort and
native binary as the bounded experiment. Time, output-token and CLI-turn budgets
are explicitly null; byte caps and infrastructure timeouts remain active. See
[configuration and execution](benchmark/README.md).

[research-unbounded-20261008.json](baselines/research-unbounded-20261008.json)
retains every attempt, frozen identities, three contrasts and source-backed
maintenance assessments. All 36 attempts have complete token telemetry. No run
timed out or failed at the transport boundary. Billing and quota cost remain
unknown.

| Arm | Completed / planned | Total reported tokens | Time including preparation | Fully satisfied outcomes |
|---|---:|---:|---:|---|
| Source tools | 12 / 12 | 4,310,046 | 50.59 min | 5 known, 7 unresolved |
| Added research instructions | 12 / 12 | 3,306,159 | 59.27 min | 6 known, 6 unresolved |
| Instructions + graph | 12 / 12 | 3,198,913 | 57.98 min | 5 known, 6 unresolved, 1 inaccurate/incomplete |

Token volume sums disjoint ordinary-input, cache-read, cache-write and output
usage reported by the runtime. Time includes preparation and run time for every
attempt. Neither measure substitutes for billing cost.

| Contrast | Token-volume change | Total-time change |
|---|---:|---:|
| Instructions vs source | −23.29% | +17.14% |
| Graph added to the same instructions | −3.24% | −2.17% |
| Instructions + graph vs source | −25.78% | +14.60% |

Negative resource changes mean less measured use. Effects differ by task:

| Task: instructions + graph vs source | Token-volume change | Total-time change | Source → graph full-key outcomes |
|---|---:|---:|---|
| Phase continuity | −39.09% | +26.03% | 0–3 → 0–3; legacy clause unresolved |
| Document evidence scope | +39.07% | +18.29% | 0–3 → 0–3; partial key coverage |
| Exact source-definition selection | −43.67% | −14.31% | 3 → 2 |
| References and removal evidence | −14.97% | +19.87% | 2–3 → 3 |

For definition selection, graph reduces raw token use by 43.67%, but its one
inaccurate answer lowers the useful-outcome yield gain to 18.34% per token and
−22.20% per unit of total time under the maintenance judgments. Across all four
tasks, unresolved outcomes bound the combined-vs-source yield gain at
−43.86% to +196.42% per token and −63.64% to +91.97% per unit of time. These are
arithmetic ranges of unresolved judgments, not confidence intervals. They do
not establish a positive overall efficiency gain.

| Review finding or limit | Treatment |
|---|---|
| Frozen phase key asks for legacy hashes in a fresh-preflight scenario | All nine outcomes stay unknown; the future public key removes the unrelated clause |
| Document answers omit deterministic IDs and the distinction between recordable endpoint drift and fatal byte errors | Correct core scope/freshness conclusions, partial key coverage; nine outcomes stay unknown |
| Final source-only reference answer omits caller count/truncation metadata | Correct dependency/removal trace; full-key outcome stays unknown |
| One graph answer says `edge_precision` is JSON null | Source serialization omits it; retain one inaccurate/incomplete outcome and its full resource cost |
| Graph-navigation wording in the old key | Public successor separates navigation from scenario/test validation; this experiment retains the frozen key |
| Citation reachability | [116 absolute Markdown targets checked](baselines/research-citations-20261008.json), zero missing/out-of-scope/invalid-line targets; this check does not prove claim support |
| Review and sampling | Implementation-agent source inspection; four public curated tasks, no independent reviewer or held-out cases |
| Runtime context and load | Served model and host isolation unverified; local fixture checks ran during part of the campaign and provider caching/load were uncontrolled |

This experiment measures static research instructions and graph navigation.
Native prompt refinement, personal profiles and accepted coding outcomes remain
unmeasured. Do not pool it with the bounded archive: key, adapter and budget
changed together. Independent key/answer review and held-out tasks are required
for a product benefit claim.

## Evidence required for feature-value claims

| Feature or outcome | Current evidence | Measurement still needed |
|---|---|---|
| Prompt refinement | Production protocol checks and static condition controls | Raw/refined accepted tasks under one original key, generation cost and native delivery |
| Personal profile | Attribution/store/replay regressions and synthetic extraction labels | Independent real-history labels, contradictions, applicability, no-profile and shuffled-profile controls |
| Project/document context | Freshness, audience, scope and coverage checks | Relevant/irrelevant/stale context contrasts on unseen tasks |
| Research and graph quality | Current four-case public calibration experiment | Held-out tasks, realistic repository discovery and independently reviewed complete answers |
| Whole workflow | Finite guards and selected CLI paths | Accepted coding outcomes, false completion, corrections and bounded failure/recovery comparisons |
| Cost and throughput | All-attempt telemetry and useful-outcome/resource formulas | Complete usage, accepted outcomes and matched budgets; subscription billing remains unknown |
| Representative indexing | Synthetic Rust call-chain fixture | Mixed-language repositories, imports, generated files and production storage/load |
| Behavioral role suites | Current loader/fixture tests | A current complete model-backed run; retained Claude observations are historical |

For comparable resource `C` and known successful outcomes `S`, resource per
success is `C / S` and useful-work yield is `S / C`. Count every planned attempt
and keep missing values unknown. Relative yield gain is undefined when baseline
yield is zero. Repetitions of four tasks do not provide 36 independent task
samples. No qualified population or causal product-quality result is available.

## Previous records

| Archive | Scope |
|---|---|
| [2026-10-07 control integration](baselines/control-loop-integration-20261007-02.json) | Earlier 75-test selection on the 3.2.1 dirty source snapshot |
| [2026-10-08 budget harness](baselines/harness-unbounded-20261008-02.json) | Earlier 358-test source snapshot, before request-bound acceptance and latency metrics |
| [2026-09-28 control integration](baselines/control-loop-integration-20260928-01.json) | Earlier clean source identity and CLI selection; retained unchanged |
| [2026-09-27 index](baselines/index-20260927.json) | Schema 1, constant-only fixture and three phases; different workload |
| [2026-10-04 local persona](baselines/persona-local-20261004.json) | Earlier replay of the same 60 synthetic hook examples |
| [Critic before prompt reduction](baselines/critic-opus-pre-lean.json) | Legacy five-case Claude capture with missing timing; not a current comparison baseline |

Retain each report's original revision, corpus, runtime, failures and limitations.
Prepare a new matched experiment after changing any relevant source, key,
instruction, model or budget. See [research instructions](benchmark/README.md)
and [review admission](benchmark/REVIEW.md).
