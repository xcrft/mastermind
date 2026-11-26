# Evaluation

This folder holds two kinds of checks. Deterministic tests check the harness and make no model calls. Model-backed trials measure answers under a pinned experiment configuration and use inference. Run every command here from the repository root.

The [scorecard](scorecard.md) lists what each retained result shows and what it leaves unmeasured. [Product success measures](PRODUCT.md) has the targets and the rules for an audit or fix case. To compare role prompts or effort settings, follow [role calibration](ROLE_CALIBRATION.md).

## Choose a package

| Package | What it checks | Instructions |
|---|---|---|
| `behavior` | Shipped roles and workflow skills on planted fixtures | [Run behavioral suites](behavior/README.md) |
| `benchmark` | Matched research trials, blinded review and paired outcome and cost analysis | [Prepare and run](benchmark/README.md), [review](benchmark/REVIEW.md) |
| `control` | Completion guards, a finite model and selected production CLI regressions | [Run the control checks](control/README.md) |
| `fixtures` | Frozen concept-retrieval queries and source anchors | [Concept relevance diagnostics](fixtures/README.md) |
| `intake` | The prompt-refiner protocol on a 40-case corpus | [Run intake evaluations](intake/README.md) |
| `persona` | Mining replay and profile delivery against a retained packet | [Persona evaluations](persona/README.md) |
| `shared` | Bounded subprocess supervision used by the other packages | [Test instructions](../tests/README.md) |

| Location | Contents |
|---|---|
| `evals/<package>/` | Runtime code and the package's own corpora and fixtures |
| `tests/evals/<package>/` | Deterministic behavioral and integration tests |
| `tests/evals/support/` | Shared disposable fixtures |
| `tests/validation/` | Repository, publication and document-evidence checks |
| `evals/baselines/` | Retained measurements with their original source bindings. Never edit them |

A report with `kind: mastermind-public-report-projection` keeps the original report's SHA-256 and lists the fields whose local paths were redacted. Its `report` field holds the published measurements and the original stays private. A projection can't be used as an input for review admission or runtime verification.

## Run checks

| Goal | Command |
|---|---|
| Python tests plus the research source binding check, with the `.venv` from `just bootstrap` | `just eval-harness` |
| Python tests only, with the system Python | `python3 -m unittest discover -s tests -t .` |
| One package | `python3 -m unittest discover -s tests/evals/benchmark -t .` |
| Research task and key source bindings | `python3 -m evals.benchmark.corpus --source-repo .` |
| Require current research source bytes | `python3 -m evals.benchmark.corpus --source-repo . --require-current` |
| Completion model only | `python3 -m evals.control --model-only --output /tmp/new-control-report` |
| Full control and CLI integration | `python3 -m evals.control --output /tmp/new-control-report` |
| Local extraction labels | `mastermind miner hooks evaluate-local --input evals/persona/local.json` |
| Index latency and memory | `just benchmark-index` |
| All behavioral suites with a model | `just evals --model sonnet` |

The Python tests use disposable fixtures and make no model calls. Full control integration needs Rust, Git and Node on a POSIX host, and a source tree nobody edits during the run. Model-backed behavioral runs use [the verified runner](behavior/README.md) and spend inference usage.

## What we measure

| Direction | Metric | How to read it |
|---|---|---|
| Useful completion | Accepted tasks out of all attempted tasks, plus user corrections | Independently judged outcomes, failed attempts included |
| Reasoning quality | Material false conclusions, required evidence coverage, correct abstention | Review complete retained answers against a fixed source key |
| Control | Completion without required proof, stale-evidence rejection, budget stops | Per-scenario results with failures. No coverage percentage from counts |
| Context efficiency | Input, cache creation and cache read tokens, p50 and p95 | Same tasks, runtime and budgets, with quality held |
| Execution cost | Total tokens, elapsed time, tool rounds, retries, reported cost | Every attempt counts, setup is separate, missing values stay unknown |
| Persona quality | Supported habits, contradictions, review and abstention rates, held-out task benefit | Git statistics and extraction volume say nothing about human traits |
| Indexing | Cold, warm and changed latency, memory, indexed and failed files | Name the corpus, machine, extraction contract and correctness checks |

A saving counts only when the quality and control criteria still hold. Savings, resource per success and yield are defined once, in [Compare paired outcomes](benchmark/REVIEW.md#compare-paired-outcomes).

| Comparison rule | Why |
|---|---|
| Keep raw cases and recompute aggregates | Nobody can report only the good runs |
| Count input, cache creation and cache read tokens | This is the context the model actually got |
| Keep reported billing cost separate | Token volume is not a bill |
| Match case and source digests, model, CLI and runtime controls | Only equal experiments compare |
| Reject a lower pass rate or a regression of a case that passed before | The quality gate comes first |
| Require lower context-token p50 and p95 | This is the runner's declared efficiency gate |
| Build new baselines from complete current evidence | The retained critic baseline has a legacy capture exception |

To run every research case under one configuration, use [the campaign commands](benchmark/README.md#run-the-whole-corpus). A model or runtime change needs a new campaign.
