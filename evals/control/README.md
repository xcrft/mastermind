# Completion control checks

Use this check before you change how the controller decides that a task iteration is complete. The controller may publish a completed iteration only when all six obligations below hold. The check proves that rule on a finite model, removes each guard to show the proof catches it, and runs exact named regressions through the production CLI, the library and the Lens UI. It makes no model calls. Retained results are in the [scorecard](../scorecard.md).

## Prerequisites

- Python 3.10+ (3.13+ on macOS), Rust, Git and Node on a POSIX host with `waitid(WNOWAIT)`.
- A source tree nobody edits during the run, because the report fails when source hashes change.
- Sources from 3.2.2 or newer for the profile-delivery regressions.

## Run the checks

From the repository root:

```sh
python3 -m evals.control --output .mastermind/research/control-loop/new-run
```

`just eval-control <new-output-directory>` runs the same command. To test another checkout with this harness, add `--source-repo /path/to/mastermind`. For the finite model alone, without Cargo or Node:

```sh
python3 -m evals.control --model-only --output /tmp/mastermind-model-new
python3 -m unittest tests.evals.control.test_harness
```

`--model-only` reports CLI and UI conformance as `not_run`, so the overall result is `incomplete`.

| Output | Contents |
|---|---|
| `report.json` | Results, starting revision, dirty-tree flag, source manifest, platform and wall time |
| Cargo stdout and stderr | Raw output per test target |
| `lens.stdout`, `lens.stderr` | The aggregate Lens DOM suite, run with `node --test --test-reporter=tap` |

The output directory must be new. Every selected test must run and pass, so a missing, ignored, renamed or failed test fails the check.

## The six obligations

| Symbol | Obligation |
|---|---|
| A | Current task contract and invocation admission |
| X | Required executor invocation completed with a valid binding |
| V | Every required observed check passed for the current inputs |
| H | Mechanical audit is Held for the recorded work |
| R | Current revision-pinned criterion review is satisfied |
| K | Current Context and Lesson decisions are resolved |

The required property is `Publish(t) => A(t) and X(t) and V(t) and H(t) and R(t) and K(t)`. `explore()` walks every reachable state of the model and checks each publishing edge against it. It then removes each guard in turn and expects a reachable violation. The model assumes intact validators, no unseen change before publication and bounded I/O, so the CLI regressions check those parts of the implementation.

## Read the report

| Field | Meaning |
|---|---|
| `bounded_model_safety` | Reachable states, publications, violations and detected guard mutants |
| `sampled_cli_conformance` | Selected production tests passed, failed or not run |
| `sampled_ui_conformance` | The Lens suite passed, failed or not run |
| `cli_cases`, `ui_suites` | Named selections, raw process outcome and elapsed time |
| `source_unchanged` | Source hashes stayed the same during the run |
| `semantic_goal_success`, `token_savings`, `cost_savings` | Always null, because nothing calls a model |

Overall `passed` needs model, CLI and UI success with unchanged sources. A nonzero exit, a timeout or a missing runtime fails the check even when a child printed a passing test summary.

The CLI selection in `harness.py` covers onboarding, required proof, acceptance, project history, recovery, admission and budget, the guarded executor, context, person profiles, hook intake, the managed worker, readiness and Lens metadata. These are sampled checks with synthetic native clients, not full coverage of code or interleavings.

## Fix a failed run

| Symptom | What to do |
|---|---|
| Output directory exists | Choose a new path |
| `source_unchanged: false` | Something edited the tree during the run. Stop the editor or use a clean checkout, then run again |
| A selected test is missing or ignored | The test was renamed or removed. Update the selection in `harness.py` together with the test |
| A selected test fails | Fix the production code. Keep the failed report |
| Profile-delivery tests missing | The source is older than 3.2.2 |

The refiner protocol has its own evaluation in the [intake package](../intake/README.md).
