# CLI platform smoke fixture

This small Python repository exercises `verify-spec` and `audit-spec` in the
[mmcg CI matrix](../../.github/workflows/ci-mmcg.yml).

The workflow copies the fixture into a temporary directory, initializes Git,
records a baseline and indexes it. `verify-spec spec.md` must pass. It then adds
a helper, records and indexes the change, and runs `audit-spec --since baseline`.
The audit accepts Held or Drift through its exit-code contract. This fixture's
planned-test heuristic is warning-only.

The smoke exercises the built binary, SQLite, Python extraction, frontmatter,
symbol lookup and Git subprocesses on each configured platform. A separate CI
step checks `init` and the JSON doctor handshake.

Keep `src/lib.py` and `spec.md` small. Rich behavioral cases belong in
[`evals/behavior/fixtures`](../../evals/README.md), where the expected contract is explicit.
