# Evaluation scorecard

## Current integration status

| Check | Status | Scope |
|---|---|---|
| Python harness | 249/249 passed | Includes 9 control-loop tests and the hook intake harness, no model calls |
| Control-loop harness | 9/9 passed, included above | Finite-model recovery/mutations, exact test accounting, process failures and combined result gates |
| Expanded CLI selection | 66/66 passed in 12 targets | Worker, readiness, event influence, intake binding, semantic follow-up, context metadata and guarded executor |
| Source-bound Lens suite | Passed | One aggregate DOM/static suite, no browser or model inference |
| Current source-bound report | [Passed](baselines/control-loop-integration-20260927-02.json) | Clean starting revision, 172 unchanged source files |
| Hook intake model trial | `not_run` | The runner exists, independent semantic label review and real processor results are still required |

Harness tests check result accounting. The retained integration run separately
records the selected production scenarios and the UI suite.

## Retained measurements

| Direction | Result | Evidence | Scope |
|---|---|---|---|
| Completion model | 65 states, 586 transitions, 0 publication violations | [Report](baselines/control-loop-integration-20260927-02.json), [contract](control-loop.md) | Six validated obligations at new publication |
| Guard sensitivity | 6/6 removed guards produce counterexamples | Same report | Mutation check of the finite model |
| Recovery | 64/64 open states reach completion within 7 successful actions | Same report | Starts a new preflight with stable inputs and successful producers |
| CLI conformance | 66/66 named scenarios pass across 12 targets | Same report | Real CLI with fixture native clients, no model calls |
| Lens boundary suite | 1 aggregate suite passed | Same report | DOM/static harness, no browser or model-use claim |
| Evaluation duration | 244.6 s | Same report | Includes build and fixture overhead |
| Indexing | Cold 1,320 ms, unchanged 233 ms, incremental 577 ms | [Raw runs](baselines/index-20260927.json), [method](../docs/benchmarks.md) | Medians of 3 runs, 1,000 Rust files, 100 changed |

The integration report records clean starting revision `f1827eb` and the complete
measured source manifest. Source hashes remained stable during the run. Raw
process logs remain local. The [previous 66-case report](baselines/control-loop-integration-20260927.json)
retains revision `9da71dd`, before the lock-lifetime fix. The earlier
[24-case report](baselines/control-loop-hooks-20260927.json)
retains its original `abf942c` dirty-tree identity. The indexing report records
`101bed7`. These records keep their original identities when history is squashed.
The different CLI selections and local build overhead prevent a timing comparison.

Generate a fresh control report after changing a measured source:

```sh
python3 -m evals.control_loop --output .mastermind/research/control-loop/run-01
```

## Unmeasured outcomes

| Direction | Existing check | Measurement still needed |
|---|---|---|
| Whole-system completion | Guard model and selected CLI paths | Rust refinement, races and independently accepted task outcomes |
| Project context | Freshness, audience and scope regressions | Retrieval relevance and task benefit on unseen work |
| Persona mining | Frozen Git replay and isolated store regressions | Human-reviewed habit precision, contradictions and held-out benefit |
| Time, tokens and cost | Telemetry and matched research-trial tooling | Current repeated end-to-end baseline with independent outcome review |
| Representative indexing | Synthetic Rust corpus | Mixed-language projects and production workloads |

`semantic_goal_success`, `token_savings` and `cost_savings` are `null` in the
control report. Test counts are not coverage percentages or evidence of zero
hallucinations.

## Historical behavioral observations

These suites have not been rerun on the current revision. The retained
[critic baseline](baselines/critic-opus-pre-lean.json) records its legacy capture
limits, including missing per-case API timing.

| Suite | Model alias | Date | Passing / attempted | First pass | Elapsed |
|---|---|---|---:|---:|---:|
| Researcher | haiku | 2026-08-25 | 3/3 | 3/3 | 36.3 s |
| Critic | opus | 2026-08-25 | 5/5 | 5/5 | 172.0 s |
| Critic before prompt reduction | opus | 2026-08-25 | 5/5 | 5/5 | 297.4 s |
| Auditor | opus | 2026-07-31 | 9/9 | 8/9 | 2,097.0 s |
| Critic | opus | 2026-07-31 | 5/5 | 5/5 | 283.0 s |
| Intake | sonnet | 2026-07-31 | 5/5 | 5/5 | 98.4 s |
| Workflow | sonnet | 2026-07-31 | 51/56 | 51/56 | 1,005.4 s |
| Workflow | sonnet | 2026-07-30 | 45/47 | 45/47 | 778.9 s |
| Workflow | sonnet | 2026-07-19 | 36/36 | 36/36 | 525.6 s |

### Critic prompt reduction

Same 5 cases, resolved model identity and Claude Code 2.1.231, 2026-08-25:

| Metric | Before | After | Change |
|---|---:|---:|---:|
| Passing cases | 5/5 | 5/5 | 0 |
| Context tokens p50 | 7,497 | 3,891 | −48.1% |
| Context tokens p95 | 7,643 | 4,037 | −47.2% |
| Suite elapsed time | 297.4 s | 172.0 s | −42.2% |

With 5 cases, nearest-rank p95 is the maximum. This historical smoke comparison
has no repeated-run variance estimate and does not measure current workflow
savings. Targeted repairs to workflow phrase assertions do not revise the
recorded 51/56 complete-suite result. Prompt-isolation changes also limit
comparisons across harness versions.

### Historical researcher telemetry

The 2026-08-25 run used three disposable Git cases with live mmcg. Its required
graph-first and source-read checks passed.

| Metric | Recorded value |
|---|---:|
| Turns | 10 |
| Output tokens | 2,772 |
| Context tokens p50 / p95 | 16,569 / 35,274 |
| Claude CLI reported cost | $0.0325 |

There is no pre-change researcher comparison. The 2026-08-11 complete rerun
stopped before inference because OAuth authentication expired. July critic
results predate that runner's prompt-isolation repair. Results before `4c338b6`
(2026-06-10) used prose verdict matching and are not comparable with current runs.

## Publish a new measurement

| Required record | Purpose |
|---|---|
| Source and case digests, tool/model identities | Identify the exact experiment |
| Runtime, environment and budgets | Establish comparable conditions |
| Every planned attempt, failure and retry | Preserve the denominator |
| Full retained answers and independent assessments | Support semantic outcome claims |
| Repeated-run distribution | Expose variance |
| Missing telemetry and experiment limits | Keep unknown values distinct from zero |

Use the [measurement definitions](README.md#measurement-contract).
