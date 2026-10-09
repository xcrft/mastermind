# Product success measures

Use these measures for code audits and fixes. The primary outcome is a result
accepted against the original user's request. Count every planned attempt,
including failures, missing answers and unresolved reviews.

Choose candidate algorithms and qualification experiments with
[Optimization research](OPTIMIZATION.md).

The [returned-range replay](baselines/retrieval-replay-20261008.json) accounts for
all sixteen retained instruction trials. It measures repeated delivery rather
than semantic redundancy; automatic removal and model savings need prospective
experiments. New Codex trials retain this accounting in adapter diagnostics.

[Stage accounting validation](baselines/codex-stage-accounting-20261008-public.json)
retains 386 passing deterministic tests and four detected guard mutations.
Codex diagnostics distinguish reported reasoning from other output, merge
overlapping tool receipt intervals and bind the raw stream digest. Summaries
keep every planned attempt, unknown captures and source bindings; campaign
conditions, repetitions and position balance must match the bound batches.

[Batched lookup validation](baselines/retrieval-implementation-20261008.json)
retains the native query comparison, Codex transport smoke, concurrency guards
and selected profile regressions. It establishes transport behavior; product
quality and model resource gains remain unmeasured.

## Baseline and initial objectives

These are starting engineering goals. Freeze them before the next run; they are
not measured benefit or a statistical qualification rule. Machine-readable
objectives and reporting live in [`benchmark/value.py`](benchmark/value.py).

The latest matched calibration compares fixed `max` with the experimental
`bounded_readonly_v1` request router. It retains `max` for workflow/removal tasks
and requests `high` for two bounded read-only tasks. Both arms use the same local
0.3.4 instruction, frozen 3.2.2 source plus pending batching/revision guard, keys,
tools and uncapped time/output-token/turn settings. There are four public tasks,
two repeats and sixteen attempts on Codex 0.162.0-alpha.2, requested `gpt-6.1-sol`.
This does not measure the published instruction or release benefit. The
subsequent [quality recheck](baselines/effort-quality-recheck-20261008.md)
adjudicates the two incomplete count explanations as unmet. It is a maintenance
review after seeing the outputs, not independent qualification.

| Measure | Current evidence | Initial objective | How to measure |
|---|---|---|---|
| Original request fulfilled | Recheck: 7/8 per arm; one paired loss and one win; review neither independent nor blinded | At least 90% accepted attempts; no paired critical-criterion loss | All request-bound criteria met, no material error or unsupported certainty |
| Correct audit or implemented fix | No material false conclusion found in these research answers; independently labeled audits/applied fixes unmeasured | Zero material errors and protected-behavior regressions in qualification | Independently labeled findings; resulting diff and regression that fails without the fix |
| Tokens per accepted task | Recheck: 361,625 → 327,725; 9.37% lower, but the paired quality gate fails | At least 10% lower with acceptance preserved | All input, cache read/write and output tokens divided by accepted outcomes |
| Total token volume | 2,531,374 → 2,294,076, 9.37% lower across all eight attempts per arm | Acceptance must be preserved before claiming useful-work savings | All planned attempts, including failures and unknown outcomes; no billing inference |
| Total latency p50 | 301.66 → 183.56 s, 39.15% lower; paired quality gate fails | At least 10% lower with acceptance preserved | Preparation + full attempt time, including failures; router bookkeeping is unmeasured |
| Total latency p95 | 540.37 → 360.53 s, 33.28% lower; descriptive observed tail | At least 10% lower with acceptance preserved | Observed sample p95, keep task families separate |
| First visible message p50 | 5.72 → 5.60 s, 2.15% lower | At least 10% lower; retain final-answer latency separately | Native launch to first completed nonempty assistant message |
| User corrections | Unmeasured | At least 20% fewer at the same acceptance rate | Corrections needed to meet the frozen request; exclude new scope |
| Relevant profile delivery | 3.2.2 fixture regressions pass; real-session fraction unmeasured | All required fitting rules delivered; whole cards and explicit omissions | Eligible/verified/offered claim IDs and revisions through actual MCP, hook and context paths |
| Automatic activation after init | Native fixture checks pass; real-session rate unmeasured | 100% of the declared supported matrix | Project/client sessions from start to completion with observed configured delivery |
| Evidence integrity | Frozen research inputs checked; served model/effort and host isolation unverified | Complete bound records; zero silent stale/foreign evidence acceptance | Original task, key, source, runtime, grants and all planned attempts |

Read the [effort-routing report](baselines/effort-routing-20261008.md) and
[sealed measurements](baselines/effort-routing-20261008.json). The frozen
quality gate failed in the recheck, so the policy stays experimental and default
effort remains unchanged. Equal average acceptance does not erase a paired
criterion loss. The original unknown judgments and the separate recheck remain
retained in [source-bound reports](baselines/effort-quality-recheck-20261008.json).

On the two changed-effort tasks, total p50 falls 264.35 → 98.83 seconds and token
volume falls 1,089,105 → 875,572. The same-max control tasks also vary: p50 falls
355.62 → 284.77 seconds. Global changes therefore cannot isolate effort, cache,
provider load or run order. Eight observations per arm and repeated attempts of
four public tasks do not establish population or causal benefit.

The [effort harness validation](baselines/effort-routing-harness-20261008.json)
retains 389 passing tests and four detected mutations: ignored effort selection,
unchecked runtime effort, discarded authentication identity and ignored risk
markers. These checks verify declared settings and evidence boundaries; they do
not establish output correctness. The request heuristic is English-only and
can miss task risk or difficulty. Use [role calibration](ROLE_CALIBRATION.md)
to compare actual roles and their runtime mappings; this study did not measure
shipped researcher, executor or auditor effectiveness.

Earlier [evidence-policy](baselines/evidence-frontier-20261008-public.json),
[retrieval-route](baselines/route-selection-20261008-public.json) and
[shorter-instruction](baselines/instruction-optimization-20261008.json)
comparisons remain separate rejected experiments. Keep their original inputs
and outcomes; do not pool them with this run. The earlier
[3.2.1 product baseline](baselines/product-20261008-03.json) uses full-source-key
outcomes rather than user acceptance. [Native 3.2.2 conformance](baselines/control-main-322-20261008-public.json)
is separate evidence.

First visible message may be commentary. It is neither time to first token nor
time to a useful final answer. Missing timing, billing, correction or activation
observations stay null. Subscription token volume is not billing cost.

## Prepare an audit/fix case

1. Retain the original request and source/diff. Define the expected result and
   protected behavior before generation.
2. Add private `acceptance_criteria`: an ID, an exact `request_excerpt` and a
   falsifiable `criterion`. Keep acceptance separate from source coverage.
3. For audits, label real defects and valid changes. Measure supported findings,
   missed material defects and false positives against independent labels.
4. For fixes, require the resulting diff and observable behavior. A regression
   must fail on the defective implementation and pass after the change. A
   proposed fix is not an accepted implemented fix.
5. Separate development cases from unseen qualification cases by repository/task
   family. Include typical, difficult and negative cases. Repeats remain one task
   sample.
6. Freeze model request/effort, binary, source, original key and condition matrix.
   Counterbalance runs; review anonymized complete outputs and calibrate model
   judgments against human judgments.

The current benchmark adapter performs read-only research. Its four public cases
have request-bound criteria for future runs. Applied fixes, user corrections and
native personalization need new bound observations; they are not inferred from
research answers.

## Isolate each feature

| Feature | Matched comparison | Additional evidence |
|---|---|---|
| Graph | Same instructions, source tools vs graph tools | Retrieval calls, tokens and accepted outcomes |
| Refinement | Original request vs original request + frozen native refinement | Refinement latency/tokens, delivery receipt, original acceptance key |
| Profile | No profile vs relevant reviewed profile vs irrelevant/shuffled profile | Scope/source/review IDs, offered cards, request precedence |
| Profile delivery fix | Old/new at 1500, then old/new at 4000 | Same eligible store; separate algorithm from increased allowance |
| Whole workflow | Same task/source, baseline vs enabled features | Accepted diff, observed checks, review and corrections |

To claim a native feature, retain its production delivery evidence. Availability
does not prove model use. Keep evaluation keys outside model inputs. Charge
preparation, refinement, retrieval, retries and failures to the appropriate arm.

## Run and inspect

```sh
python3 -m unittest discover -s tests -t .
python3 scripts/validate.py

# Test current sources while keeping local benchmark edits separate.
python3 -m evals.control --source-repo /path/to/3.2.2-checkout \
  --output .mastermind/research/control/new-run

# Read sealed results without invoking a model.
python3 -m evals.benchmark.value \
  --review-set .mastermind/research/campaign/reviews \
  --baseline source --candidate portable_mmcg \
  --output .mastermind/research/product/new-report.json

# Inspect context delivery across every planned attempt.
python3 -m evals.benchmark.retrieval_analysis /path/to/campaign \
  --output .mastermind/research/product/new-retrieval.json
```

See [trial preparation](benchmark/README.md) and
[review admission](benchmark/REVIEW.md). Output paths must be new. Preserve failed
runs, fix the owning input or implementation and prepare a new run. Do not edit
a retained key or remove failed attempts.

The [current read-contract report](baselines/source-pagination-20261008.json)
records 372 passing tests, four detected production mutations and the source
snapshot. Source reads return bounded chunks with explicit continuations.
Replaying 674 recorded requests resolves all 71 prior read failures; following
the continuations reproduces every complete requested range. A real Codex smoke
read all 390 requested lines in two replies. This verifies transport and read
completeness; model latency/quality gain from pagination remains unmeasured.

| Report condition | Interpretation |
|---|---|
| Request outcomes unresolved | Bounds; no point efficiency or target claim |
| Any paired outcome loss | Quality regression, even if token use falls |
| Zero accepted baseline | Relative useful-work efficiency undefined |
| Missing timing/usage | Measured/unknown denominators; no survivor-only comparison |
| Goals met on public cases | Descriptive result; independent unseen review still required |
| Legacy source-key review | Coverage proxy; cannot be relabeled as user acceptance |

## Preserve the profile fixes

The control selection includes four `profile_budget_cli` tests and twelve exact
library regressions for ranking, partial delivery, eligibility/caps, context
fitting, settings and legacy configuration. Missing, renamed, ignored or failed
tests fail the selection. Test 3.2.2 or newer sources; 3.2.1 cannot satisfy this
gate.

Before optimizing delivery, run these regressions and demonstrate sensitivity:
revert only the owning production behavior in a private copy, retain the tests,
record their expected failure, then restore exact source bytes. Preserve these
boundaries when reducing context.

[Four retained production mutations](baselines/profile-regression-mutations-20261008.json)
were detected: whole-list removal, the twelve-card cap, missing ranking and an
ignored init budget. Source bytes were restored. This is sensitivity evidence
for these selected guards, not a mutation-coverage percentage.

[Three product-report guard mutations](baselines/product-guard-mutations-20261008.json)
were detected: inconsistent acceptance, a bypassed quality gate and latency
outside the observed process span. The tests use controlled observations and
isolated copies without model calls.

The task-specific criteria and human calibration follow
[OpenAI's evaluation guidance](https://developers.openai.com/api/docs/guides/evaluation-best-practices).
