# Python tests

These tests cover the eval harnesses, process supervision and the repository's
release and audit contracts. They use temporary Git and SQLite repositories and
fake processors, and make no model calls. Run every command from the repository
root, after `just bootstrap` once for the pinned validator dependencies.

| Goal | Command |
|---|---|
| All Python tests and the eval corpus check | `just eval-harness` |
| Python tests only, without just | `python3 -m unittest discover -s tests -t .` |
| One package | `python3 -m unittest discover -s tests/evals/benchmark -t .` |
| One test | `python3 -m unittest tests.evals.benchmark.test_trials.BenchmarkTests.test_batch_enforces_recorded_order_without_spending_a_later_attempt` |

| Directory | Covers |
|---|---|
| `evals/behavior` | Role outputs, case and report contracts, prompt isolation |
| `evals/benchmark` | Frozen trials, adapters, source bindings and offline review |
| `evals/control` | Finite completion model and integration-result accounting |
| `evals/intake` | Refiner envelopes, processor failures and retained evidence |
| `evals/persona` | Git attribution and replay corpus membership |
| `evals/shared` | Real process supervision, pipe limits and strict JSON parsing |
| `evals/support` | Shared fixtures: disposable repositories, fake runtimes and process-state helpers |
| `validation` | Release and audit workflow contracts, packaging, the validator, and the project-history helper of [mastermind-project-history](../skills/workflow/mastermind-project-history/SKILL.md) |
| `ci-fixture` | The repository CI runs its end-to-end smoke against |

POSIX process checks need `ps` and `waitid(WNOWAIT)`, which on macOS means
Python 3.13 or later. Git-backed tests need Git, and a skipped platform check
says nothing about that platform. Model-backed trials have their own commands in
the [eval instructions](../evals/README.md).

## Add a regression

1. Put the test beside the package it exercises. Shared setup and cleanup go in
   a fixture under `tests/evals/support/`, which doesn't inherit from
   `unittest.TestCase`.
2. Assert the resulting behavior and retained artifacts, and cover the failure
   boundary instead of repeating an existing happy path.
3. For subprocesses, wait for readiness before you advance a test clock, and
   check cleanup by process state. A short elapsed-time threshold is flaky.
4. Revert or mutate the protected behavior and check that the test fails.
5. Run the affected package and `python3 -m unittest discover -s tests -t .`.
