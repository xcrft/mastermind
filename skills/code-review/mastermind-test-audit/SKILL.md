---
name: mastermind-test-audit
description: Read-only review of whether the tests in a finished change actually prove its behaviour — changed code no test reaches, a test exercising a different path than the one that changed, an assertion edited to match the new output, and a suite that ran nothing. Use after implementation, and whenever a change claims to be covered.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - code-review
    - testing
    - qa
    - mmcg
---

# Mastermind test audit

A green suite proves that the assertions present passed. It does not prove the
change is covered, and it does not prove the assertions were checking the thing
that changed. Those are different claims, and only the first one runs
automatically.

You are read-only. You report findings and change nothing.

## What the controller already established

Do not re-derive these — read them and build on them:

- **`vacuous_test_claim`** in `audit.md` means a verification command provably
  ran zero tests. The controller checks `go test`, `pytest`, `cargo test`, and
  the jest / vitest / `npm test` family, and it stays silent when the answer is
  undeterminable rather than guessing.
- **`missing_test`** means a test the spec planned is absent.
- **`signature_changed`** names symbols whose contract moved.

If `audit.md` is absent, say so and continue on the diff and the graph. Do not
present the controller's findings as your own, and do not contradict them
without new evidence.

## The four checks

### 1. Changed behaviour no test reaches

`mmcg_test_impact --since <baseline>` returns candidates per changed symbol with
a classification that is the whole point:

- `direct` — the test is a changed test symbol, or reaches the change at graph
  depth 1. This is coverage evidence.
- `transitive` — reaches it at depth ≥2. Weaker: the test may pass while the
  changed line never executes.
- `heuristic` — a filename matched. **This is not coverage.** It is a place to
  look, and treating it as proof is the mistake this check exists to catch.

A changed symbol with no `direct` candidate is uncovered behaviour. Say which
symbol and what classification you did find; "there are tests nearby" is not an
answer.

### 2. The test exercises a different path than the change

A suite can be green, on-topic, and still never touch the changed code. Compare
what the test calls with what production calls: `mmcg_callees` on the test
symbol, `mmcg_callers` on the changed symbol. When the test calls a wrapper,
a helper, or an older entry point that the real caller no longer uses, the green
result is about a path nobody runs.

Name both paths — the one the test takes and the one production takes.

### 3. An assertion edited to match the new output

When the implementation and its assertion change in the same diff, the test
stopped being an independent check of behaviour and became a restatement of the
code. Sometimes that is correct — the expected behaviour genuinely changed and
the contract moved with it. Sometimes it is how a real regression ships green.

Report the pair and say which the change claims to be. This is the one check
with no tool behind it: it is visible only by reading the diff on both sides.

### 4. A test that asserts nothing

A test body that calls the code and never asserts on the result passes as long
as nothing throws. It is a smoke test wearing a behaviour test's name. Flag it
when the change added or edited it, and quote the body.

## Evidence rule

**A finding names the symbol, the test, and the query result behind it.**
"`createOrder` is untested" is a claim; "`mmcg_test_impact` returns one
`heuristic` candidate for `createOrder` and no `direct` one" is a finding.
Without the query, downgrade to `could_not_verify` or drop it.

## Restraint

Finding nothing is a normal outcome. A change whose behaviour has direct tests
that exercise the production path is a clean result, and reporting it as clean
is correct. Do not manufacture coverage findings — a reviewer that always wants
one more test is one nobody reads.

## What this review cannot judge

- **Coverage is not correctness.** A `direct` test proves the code ran, not that
  the expected value is right.
- **Flakiness, ordering, and timing** are runtime properties and are invisible
  here.
- **Whether the suite passed** is the executor's observation, not yours. Re-run
  only cheap deterministic commands, and mark anything expensive or
  environment-dependent `not_rerun` rather than describing it as verified.

## Output

````markdown
## Test audit: <clean | findings>

### Uncovered behaviour
- `<symbol>` at `<file>:<line>` — candidates: <classification list, or none>

### Wrong path exercised
- `<test>` calls `<what the test reaches>`; production reaches `<changed symbol>` via `<caller>`

### Assertion moved with the code
- `<test>:<line>` — assertion changed alongside `<symbol>` in the same diff

### Non-asserting tests
- `<test>:<line>` — `<body>`

### Kept
- <coverage the change added or preserved that genuinely proves behaviour>

### Could not verify
- <claim> — <which query was inconclusive and why>

<!-- mastermind:test-audit-begin -->
```yaml
baseline: <ref>
verdict: clean | findings
symbols_changed: <N>
symbols_with_direct_tests: <N>
flagged: <N>
findings:
  - symbol: <name>
    test: <path or none>
    kind: uncovered | wrong_path | assertion_moved | non_asserting | could_not_verify
```
<!-- mastermind:test-audit-end -->
````

`flagged` counts every `findings` entry. `verdict` is `findings` when `flagged`
is above zero and `clean` otherwise. Keep every heading even when empty, and
never omit the sentinel block.

## Boundaries

- Read-only. Never edit source, tests, reports, `audit.md`, `_lessons.md`, or
  `state.json`; never stage, commit, or revert.
- Report findings; do not write the missing tests.
- This is not a contract audit. It produces no `held` / `drift` / `broken`
  verdict and does not replace the controller's post-flight.
