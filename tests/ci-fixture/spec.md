---
id: "ci-smoke"
title: CI smoke fixture
risk: low
touches:
  - file: src/lib.py
    language: python
    symbols:
      - name: greet
        signature: "def greet(name: str) -> str"
        callers: 1
verify:
  - cmd: "true"
expected_docs: []
breaking_changes:
  removed_symbols: []
---

# CI smoke fixture

Trivial spec consumed by `mmcg verify-spec` and `mmcg audit-spec` in CI.
Designed to pass `verify-spec` cleanly at baseline; `audit-spec` runs on the
post-edit state (a new helper appended by the CI workflow) which counts as
benign scope creep — verdict should be `Drift`, not `Broken`, since nothing
in the snapshot was removed silently and the post-edit file is the one
declared in `touches`.

## Goals

1. Be a no-op gate the CI runs on every PR across all 7 build-matrix targets.

## Alternatives Considered

- Inline HEREDOC in workflow yaml — rejected, harder to read and maintain across changes to the spec schema.

## Tests Plan

- `test_smoke_passes` — implicit (CI exit code 0 is the assertion).

## Documentation Plan

- `tests/ci-fixture/README.md` explains the fixture role.

## Observability Plan

- N/A — fixture is exercised only in CI.

## Performance Considerations

- O(1) — single file, two functions, two module constants.
