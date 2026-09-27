# Completion contract and executable evaluation

This evaluation checks publication of a new completed iteration.

| Layer | Check | Recorded result |
|---|---|---|
| Finite model | Every reachable publication requires all six obligations | 65 states, 586 transitions, 0 violations |
| Mutation detection | Remove each guard and require a reachable counterexample | 6/6 detected |
| CLI conformance | Run exact named regressions through the production CLI | 24/24 passed across 8 targets |

Results: [2026-09-27 report with hook intake](baselines/control-loop-hooks-20260927.json).
The proof applies to the finite model. Full Rust refinement and arbitrary goal
achievement remain outside the claim.

## Run

From the repository root, with Python 3.10+, Rust and Git on a POSIX host:

```sh
python3 -m evals.control_loop --output .mastermind/research/control-loop/run-01
```

| Run requirement | Behavior |
|---|---|
| Output directory | Must be new |
| Native clients | Fixtures in disposable repositories, no model calls |
| Selected tests | Every exact test must run and pass, missing/ignored/renamed/failed tests fail the eval |
| Source binding | Hashes before and after execution must match |
| Output | `report.json` plus raw Cargo stdout/stderr per target |
| Report provenance | Starting revision, dirty-tree flag, source manifest, platform and wall time |
| Trust boundary | Owner-writable local records, no runner signature or model-quality assessment |

For the finite model alone:

```sh
python3 -m evals.control_loop --model-only --output /tmp/mastermind-model-01
python3 -m unittest evals.test_control_loop
```

The first command can succeed while the overall report remains `incomplete` and
CLI conformance remains `not_run`. CI runs the model tests in the Python harness
and the integration cases in the Rust suite. The combined report command is
available as `just eval-control <new-output-directory>`.

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
| Recovery | Bound follow-up instructions, repeated review without another execution |
| Admission and budget | Native denial, controller lock, stale input, finite repair attempts |
| Context | Stale documents withheld, audience and source scope enforced |
| Person profile | Collection creates no active habit, acceptance stays bound to reviewed sources |
| Hook intake | Invalid results, timeouts, replay, revocation, concurrent prompts and unfinished deliveries cannot publish a workflow handoff |

These tests exercise production CLI paths. They are sampled conformance
evidence: passing them does not prove that the abstraction covers all code,
interleavings, inputs or supported environments. Persona, context and hook tests are
adjacent boundary checks, not part of the six-bit completion theorem.

## Reading results

| Field | Meaning |
|---|---|
| `bounded_model_safety` | Reachable states, publications, violations and detected guard mutants |
| `sampled_cli_conformance` | Exact selected production tests passed, failed or did not run |
| `source_unchanged` | Recorded source inventory and hashes stayed the same during the run |
| `elapsed_seconds` | Evaluation duration on this machine, including build and fixture overhead |
| `semantic_goal_success` | Unmeasured by this evaluation |
| `token_savings`, `cost_savings` | Unmeasured, there are no inference calls |

Real-task false completion, user corrections, unsupported conclusions, tokens
and time require a fixed, independently reviewed task corpus. See
[measurement definitions](README.md#measurement-contract) and the
[current scorecard](scorecard.md).
