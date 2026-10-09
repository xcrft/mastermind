# Tests

Run commands from the repository root. Install the pinned validator dependencies
once with `just bootstrap`.

| Goal | Command |
|---|---|
| All Python tests | `just eval-harness` |
| Without just | `python3 -m unittest discover -s tests -t .` |
| One eval package | `python3 -m unittest discover -s tests/evals/benchmark -t .` |
| Repository/publication contracts | `python3 -m unittest discover -s tests/validation -t .` |
| One regression | `python3 -m unittest tests.evals.benchmark.test_trials.BenchmarkTests.test_batch_enforces_recorded_order_without_spending_a_later_attempt` |

| Directory | Responsibility |
|---|---|
| `evals/behavior` | Role outputs, case/report contracts and prompt isolation |
| `evals/benchmark` | Frozen trials, adapters, source bindings and offline review |
| `evals/control` | Finite completion model and integration-result accounting |
| `evals/intake` | Refiner envelopes, processor failures and retained evidence |
| `evals/persona` | Git attribution and replay corpus membership |
| `evals/shared` | Real process supervision and pipe limits |
| `evals/support` | Disposable repositories, fake runtimes and process-state helpers |
| `validation` | Audit publication, packaging and document evidence |

Tests use temporary Git/SQLite repositories and explicit fake processors. They
make no model calls. POSIX process checks require `ps` and `waitid(WNOWAIT)`
(Python 3.13+ on macOS); Git-backed tests require
Git. A skipped platform check is not runtime evidence for that platform.

## Add a regression

1. Put the test beside the package it exercises. A shared fixture owns setup and
   cleanup; it does not inherit from `unittest.TestCase`.
2. Assert the resulting behavior and retained artifacts. Include the relevant
   failure boundary instead of duplicating an existing happy path.
3. For subprocesses, wait for readiness before advancing a test clock. Check
   cleanup by process state rather than a short elapsed-time threshold.
4. Confirm that reverting or mutating the protected behavior makes the test fail.
5. Run the affected package and the shared discovery command used by CI.

Model-backed trials and full Rust/Node integration have separate commands in
[eval instructions](../evals/README.md). Their results are not produced by this
test suite.
