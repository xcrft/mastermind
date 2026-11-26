# Product success measures

We judge a code audit or fix by one primary outcome: a result accepted against the original user's request. Every planned attempt counts, including failures, missing answers and unresolved reviews. Retained results for each measure are in the [scorecard](scorecard.md).

## Initial targets

These are engineering targets to freeze before the next run. They are not a measured benefit and not a statistical qualification rule. [`benchmark/value.py`](benchmark/value.py) holds the machine-readable targets in `OBJECTIVES` and reports them for a sealed review set.

| Measure | Initial target | How to measure |
|---|---|---|
| Original request fulfilled | At least 90% accepted attempts, no paired loss on a critical criterion | Every request-bound criterion met, no material error or unsupported certainty |
| Correct audit or implemented fix | Zero material errors and zero protected-behavior regressions in qualification | Independently labeled findings, the resulting diff, and a regression test that fails without the fix |
| Tokens per accepted task | At least 10% lower with acceptance preserved | All input, cache read, cache write and output tokens, divided by accepted outcomes |
| Total token volume | No saving claim until acceptance is preserved | Every planned attempt, failures and unknown outcomes included. No billing estimate |
| Total latency p50 and p95 | At least 10% lower with acceptance preserved | Preparation plus the full attempt, failures included. Keep task families separate for p95 |
| First visible message p50 | At least 10% lower. Keep final-answer latency separate | Native launch to the first completed nonempty assistant message |
| User corrections | At least 20% fewer at the same acceptance rate | Corrections needed to meet the frozen request, new scope excluded |
| Relevant profile delivery | Every fitting required rule delivered as a whole card, with explicit omissions | Eligible, verified and offered claim IDs and revisions through the real MCP, hook and context paths |
| Automatic activation after init | 100% of the declared supported matrix | Project and client sessions from start to finish with observed configured delivery |
| Evidence integrity | Complete bound records, zero silent acceptance of stale or foreign evidence | Original task, key, source, runtime, grants and every planned attempt |

The first visible message can be commentary, so it is neither time to first token nor time to a useful answer. Missing timing, billing, correction or activation observations stay null. Subscription token volume is not a billing cost.

| Report condition | How to read it |
|---|---|
| Request outcomes unresolved | Bounds only, no point efficiency or target claim |
| Any paired outcome loss | A quality regression, even when token use falls |
| Zero accepted baseline | Relative useful-work efficiency is undefined |
| Missing timing or usage | Measured and unknown denominators, no survivor-only comparison |
| Targets met on public cases | A descriptive result. Independent unseen review is still needed |
| Legacy source-key review | A coverage proxy that can't be relabeled as user acceptance |

To read a sealed review set against these targets without a model call, run from the repository root:

```sh
python3 -m evals.benchmark.value \
  --review-set .mastermind/research/campaign/reviews \
  --baseline source --candidate portable_mmcg \
  --output .mastermind/research/product/new-report.json
```

The output path must be new. Keep failed runs, fix the owning input or code and prepare a new run. Never edit a retained key or drop failed attempts.

## Prepare an audit/fix case

1. Keep the original request and the source or diff. Define the expected result and the protected behavior before generation.
2. Add private `acceptance_criteria`, each with an ID, an exact `request_excerpt` and a falsifiable `criterion`. Keep acceptance separate from source coverage.
3. For an audit, label real defects and valid changes. Measure supported findings, missed material defects and false positives against independent labels.
4. For a fix, require the resulting diff and observable behavior. The regression test must fail on the defective code and pass after the change. A proposed fix is not an implemented fix.
5. Split development cases from unseen qualification cases by repository or task family. Include typical, difficult and negative cases. Repeats of one task stay one task sample.
6. Freeze the model request and effort, the binary, the source, the original key and the condition matrix. Counterbalance the run order, review anonymized complete outputs and calibrate model judgments against human ones, as [OpenAI's evaluation guidance](https://developers.openai.com/api/docs/guides/evaluation-best-practices) suggests.

The current benchmark adapter only does read-only research. Its four public cases have request-bound criteria for future runs. Applied fixes, user corrections and native personalization need new bound observations, because research answers can't stand in for them.

To claim a native feature, keep its production delivery evidence, since a tool being available doesn't mean the model used it. Keep evaluation keys out of model inputs and charge preparation, refinement, retrieval, retries and failures to the arm that caused them. The matched comparison for each feature is in the [scorecard](scorecard.md#not-measured-yet).

## Preserve the profile fixes

The [control checks](control/README.md) select four `profile_budget_cli` tests and twelve exact library regressions for ranking, partial delivery, eligibility and caps, context fitting, settings and legacy configuration. Run them before any change to profile delivery. To test the delivery algorithm, compare old and new code at a 1500 allowance first and only then at 4000, so the algorithm change stays separate from the bigger allowance.

To show a guard is sensitive, revert only the owning production behavior in a private copy, keep the tests, record the expected failure, then restore the exact source bytes. The retained [profile delivery mutations](baselines/profile-regression-mutations-20261008.json) and [product-report mutations](baselines/product-guard-mutations-20261008.json) did this for the guards they name.
