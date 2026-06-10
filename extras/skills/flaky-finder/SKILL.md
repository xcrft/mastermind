---
name: flaky-finder
description: Identify flaky tests by running the suite repeatedly and bisecting failures across runs. Use when the user says "find flaky tests", "this test is flaky", "tests pass locally but fail in CI", or sees intermittent test failures.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - testing
  model: sonnet
---

# Flaky Test Finder

Finds tests that pass and fail non-deterministically. Runs the suite N times, records which tests changed outcome between runs, and ranks them by flake rate.

## When to use

- User reports "tests pass locally but fail in CI"
- A test failed once and the user wants to confirm if it's flaky before retrying
- User explicitly asks for a flake audit before a release
- Do NOT use for finding *broken* tests — those fail consistently. Use a regular test run for that.

## Prerequisites

- A working `<test-command>` for the project (`pytest`, `go test ./...`, `npm test`, etc.)
- Time — flake hunting is inherently slow (10-50 runs of the full suite)

## Steps

1. **Confirm the test command.** Read the project's CI config or `package.json` / `Makefile`. If unclear, ask.
2. **Establish a baseline.** Run the suite once. If it fails, the failures aren't flakes — they're broken. Stop and report.
3. **Decide N.** Default to 20 runs. For long suites (>5min), drop to 10. For fast suites (<30s), go to 50.
4. **Run N times, recording each test's pass/fail per run.** Use the project's machine-readable output if available (`pytest --junitxml`, `go test -json`, `jest --json`).
5. **Compute flake rate per test.** A test that passed 18/20 times and failed 2/20 has a flake rate of 10%.
6. **Rank by flake rate descending.** Anything between 1% and 99% is suspicious; 0% and 100% are deterministic.
7. **For the top 3 flakiest, read the test code.** Look for: shared state, time-based assertions, network calls, ordering assumptions, race conditions.
8. **Report findings.**

## Outputs

```markdown
## Flake report — N=<N> runs of <test-command>

### Flaky tests (sorted by flake rate)
| Test | Flake rate | Likely cause |
|---|---|---|
| `tests/limiter_test.go::TestConcurrentBucket` | 35% (7/20 failed) | Race on shared counter, no `t.Parallel()` synchronization |
| `tests/api_test.py::test_response_time` | 15% (3/20 failed) | Time-based assertion `< 100ms` — fails under load |

### Deterministic failures
- `tests/foo_test.py::test_bar` — failed all 20 runs. Not a flake; this test is broken.

### Deterministic passes
- <count> tests passed all <N> runs.
```

## Examples

**Input:** "Our CI is flaky, can you find the culprit?"

**Output (abbreviated):**
```markdown
## Flake report — N=20 runs of `pytest tests/`

### Flaky tests
| Test | Flake rate | Likely cause |
|---|---|---|
| `test_websocket_reconnect` | 25% | Race between `await ws.connect()` and the heartbeat loop |
| `test_cache_eviction` | 5% | Wall-clock assertion `time.time() - start < 1.0` |

### Deterministic
- 312 passed all 20 runs
- 0 failed all 20 runs
```
