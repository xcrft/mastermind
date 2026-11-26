# Evaluation

This folder holds the checks behind Mastermind's quality and cost claims. Deterministic tests check the harness and make no model calls. Model-backed runs measure real answers and spend inference usage. Run every command from the repository root. The [scorecard](scorecard.md) lists each retained result and what it leaves unmeasured.

| Package | What it checks | Runbook |
|---|---|---|
| `behavior` | Shipped roles and workflow skills on planted cases | [Behavioral evaluations](behavior/README.md) |
| `benchmark` | Matched research trials, blinded review and paired cost analysis | [Research benchmark](benchmark/README.md), [offline review](benchmark/REVIEW.md) |
| `control` | Completion guards, a finite model and selected CLI regressions | [Completion control checks](control/README.md) |
| `fixtures` | Frozen concept-retrieval queries and source anchors | [Concept relevance fixtures](fixtures/README.md) |
| `intake` | The prompt-refiner protocol on 40 cases | [Refiner evaluation](intake/README.md) |
| `persona` | Mining replay and profile delivery | [Persona evaluations](persona/README.md) |
| `shared` | Bounded subprocess supervision for the other packages | [Test instructions](../tests/README.md) |

Tests live in `tests/evals/<package>/`, shared disposable fixtures in `tests/evals/support/`, and repository checks in `tests/validation/`.

## Run the checks

| Goal | Command |
|---|---|
| Python tests and the research corpus check, with the `.venv` from `just bootstrap` | `just eval-harness` |
| Python tests with the system Python | `python3 -m unittest discover -s tests -t .` |
| One package | `python3 -m unittest discover -s tests/evals/benchmark -t .` |
| Research corpus source bindings | `python3 -m evals.benchmark.corpus --source-repo . --require-current` |
| Completion model only | `python3 -m evals.control --model-only --output /tmp/new-control-report` |
| Full control and CLI integration | `python3 -m evals.control --output /tmp/new-control-report` |
| Local extraction labels | `mastermind miner hooks evaluate-local --input evals/persona/local.json` |
| Index latency and memory | `just benchmark-index` |
| All behavioral suites with a model | `just evals --model sonnet` |

In an agent session, run them outside the Bash sandbox, because the control and process tests bind local ports and manage process groups. Full control integration also needs Rust, Git and Node.

## Keep retained results intact

`evals/baselines/` holds retained measurements with their original source bindings, so never edit a file there. To correct a result, run a new measurement and write a new file. A report with `kind: mastermind-public-report-projection` is a public copy of a private report. It keeps the original's SHA-256 and lists the fields whose local paths were redacted, and it can't be used as input for review or runtime checks.

A comparison counts only when both arms used the same cases, source digests, model, CLI and budgets, and every planned attempt stays in the totals, failures included. A saving counts only when acceptance holds. The metrics are defined in [Compare paired outcomes](benchmark/REVIEW.md#compare-paired-outcomes).
