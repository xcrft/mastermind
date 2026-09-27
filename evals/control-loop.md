# Completion contract and executable evaluation

This evaluation checks publication of a new completed iteration.

| Layer | Check | Retained result |
|---|---|---|
| Finite model | Every reachable publication requires all six obligations | 65 states, 586 transitions, 0 violations |
| Mutation detection | Remove each guard and require a reachable counterexample | 6/6 detected |
| CLI conformance | Run exact named regressions through the production CLI | 66/66 passed across 12 targets |
| Lens UI | Check privacy, coverage and delivery metadata | 1 aggregate DOM/static suite passed |

The [2026-09-27 integration report](baselines/control-loop-integration-20260927.json)
records a clean starting revision and 172 unchanged source files. The run took
254.3 seconds, including local build and fixture overhead. The proof applies to
the finite model. It does not prove full Rust refinement or arbitrary goal achievement.

## Run

From a stable source tree, with Python 3.10+, Rust, Git and Node on a POSIX host:

```sh
python3 -m evals.control_loop --output .mastermind/research/control-loop/run-01
```

| Run requirement | Behavior |
|---|---|
| Output directory | Must be new |
| Native clients | Fixtures in disposable repositories, no model calls |
| Selected tests | Every exact test must run and pass, missing/ignored/renamed/failed tests fail the eval |
| Lens UI | One aggregate DOM/static suite via `node --test --test-reporter=tap`, not a browser run |
| Source binding | Hashes before and after execution must match |
| Output | `report.json`, raw Cargo stdout/stderr per target and `lens.stdout`/`lens.stderr` |
| Report provenance | Starting revision, dirty-tree flag, source manifest, platform and wall time |
| Trust boundary | Owner-writable local records, no runner signature or model-quality assessment |

For the finite model alone:

```sh
python3 -m evals.control_loop --model-only --output /tmp/mastermind-model-01
python3 -m unittest evals.test_control_loop
```

The first command leaves CLI and UI conformance `not_run` and the overall report
`incomplete`. The second checks the model and harness accounting without running
Cargo, Node or a model. The combined source-bound report is also available as
`just eval-control <new-output-directory>`.

## State and assumptions

For one structured task iteration, let its obligations at publication time be:

| Symbol | Obligation | Boundary |
|---|---|---|
| A | Current task contract and invocation admission | Does not imply every OS effect was authorized |
| X | Required executor invocation completed with valid binding | Manual workflows have no native-invocation requirement |
| V | Every required observed check passed for current bound inputs | External environments are not fully captured |
| H | Mechanical audit is Held for the recorded work | Does not supply semantic acceptance |
| R | Current revision-pinned criterion review is satisfied | Judgment truth is an assumption |
| K | Current canonical Context/Lesson decisions are resolved | Reviewed bytes and decisions are explicit |

The state has six validation bits and one completion flag. A true obligation
means its associated records passed validation. Scope, identity, index
availability and freshness are abstracted inside each bit.

| Proof assumption | Implementation obligation outside this model |
|---|---|
| Contract is true and sufficient | Define criteria that capture the user's outcome |
| Records and validators are intact | Validate identity, schema, scope and current source bytes |
| No unseen mutation before publication | Establish consistent validation/publication under concurrency |
| Bounded I/O terminates | Enforce process and storage bounds |
| Evidence owner is trusted | Hashes alone cannot detect an owner rewriting the entire chain |

## Safety

Let `Publish(t)` mean that the controller newly records iteration `t` as
completed. The required property is:

```text
Publish(t) => A(t) and X(t) and V(t) and H(t) and R(t) and K(t)
```

Proof in the finite model:

1. The initial state is open.
2. Execution, checks, audit and review actions do not publish completion.
3. Invalidation actions can remove an obligation while leaving other old
   records present. A new preflight resets the previous evidence.
4. The only publishing transition requires all six obligations.
5. Therefore every publishing edge from a reachable state satisfies the rule.

`explore()` enumerates reachable states and checks every publishing edge against
a separate invariant predicate. It removes each guard in turn and requires a
reachable violating publication. Missing counterexamples fail the mutation check.

The invariant concerns the moment of publication. Completed iterations retain
their historical meaning when later work changes a file. An explicit re-audit
starts fresh evidence validation.

## Progress and stopping

| Property | Argument | Limit |
|---|---|---|
| Conditional reachability | Every open model state can start a new preflight and complete in ≤7 successful actions | Assumes stable inputs and successful producers, says nothing about repairing the current iteration |
| Bounded automatic repair | `remaining = max_iterations - iteration` is nonnegative and decreases per permitted retry | Requires terminating I/O and bounded child processes |
| Budget exhaustion | Controller stops the automatic sequence | The task remains unresolved |
| Manual retry or new contract | Starts a new sequence | No global convergence guarantee |

The seven actions are preflight, execution, verification, audit, criterion review,
history review and publication.

## Connection to the implementation

The source-bound CLI selection in `control_loop.py` covers:

| Direction | Observable scenarios |
|---|---|
| Required proof | Missing or failed checks, foreign-repository receipt, late executable change |
| Acceptance | Positive completion, unknown/negative judgments, review compare-and-swap |
| Project history | Required updates block completion, changed lessons remain reviewable |
| Recovery | Bound follow-up instructions, review without execution, one opted-in semantic retry with fresh evidence |
| Admission and budget | Native denial, controller lock, stale input, shared finite retry budget |
| Guarded executor | Supported fixture calls, denied paths/actions, missing or conflicting mediation, receipt replay and declared check commands |
| Context | Current document evidence, exact bytes offered to the process, audience scope and private review metadata |
| Person profile | Collection creates no active habit, authorship review and current sources remain required |
| Observation influence | Original observations remain available, later profile/refiner exposure limits habit promotion |
| Hook intake | Invalid results, replay, revocation, unfinished deliveries, session binding and compare-and-swap recovery |
| Managed worker | Single owner, persistent budgets/checkpoints, client isolation, restart, stop, timeout and source drift |
| Readiness | Registration, capture, observed sessions, refiner configuration and worker state remain separate |
| Lens UI | Private metadata clears on leave/error, stays out of standalone export and distinguishes offered context from unknown model use |

CLI selectors exercise production paths with synthetic native clients and
processors. Lens uses a mocked DOM. The harness counts that file as one aggregate
suite, not one independent test for each helper function. These are sampled
conformance checks, not exhaustive coverage of code, interleavings or environments.

Persona, context, intake, worker, readiness, guarded execution and UI checks are adjacent boundaries.
They do not add obligations to the six-bit theorem or establish model benefit,
native before-effect enforcement or verified native loading.

## Reading results

| Field | Meaning |
|---|---|
| `bounded_model_safety` | Reachable states, publications, violations and detected guard mutants |
| `sampled_cli_conformance` | Exact selected production tests passed, failed or did not run |
| `sampled_ui_conformance` | Aggregate Lens suite passed, failed or did not run |
| `cli_cases`, `ui_suites` | Named selections, raw process outcome, elapsed time and accounting |
| `source_unchanged` | Recorded source inventory and hashes stayed the same during the run |
| `elapsed_seconds` | Evaluation duration on this machine, including build and fixture overhead |
| `semantic_goal_success` | Unmeasured by this evaluation |
| `token_savings`, `cost_savings` | Unmeasured, there are no inference calls |

Real-task false completion, user corrections, unsupported conclusions, tokens
and time require a fixed, independently reviewed task corpus. See
[measurement definitions](README.md#measurement-contract) and the
[current scorecard](scorecard.md).

Overall `passed` requires model, CLI and UI success with unchanged sources. A
nonzero exit, output/timeout stop or missing runtime cannot become a passing
result because a child printed a successful test summary.

## Refiner protocol and model evaluation

The 40-case multilingual corpus can run through the production refiner parser:

```sh
python3 -m evals.hook_intake --binary /absolute/path/to/mmcg \
  --processor /absolute/path/to/protocol-processor \
  --output /private/new-report-directory
```

| Output | Contract |
|---|---|
| All attempts | Retained request, bounded raw streams, status and digests, including failures |
| Protocol | Production parsing with `admission: false`, no capture or task publication |
| Labels | Hidden from the processor. Agreement is against author-written synthetic labels |
| Bounds | 1–5 repetitions, 1–20 s native timeout, processor budget 0.25 s shorter for recorder finalization |
| Independent review | Missing until a separate label review is supplied |
| Meaning, cost and benefits | Require real provider runs and reviewed outcomes. Fixture success establishes wiring only |

Select a protocol executable explicitly. The runner does not select credentials
or call a provider by default. Repeat `--processor-arg=VALUE` as needed.
