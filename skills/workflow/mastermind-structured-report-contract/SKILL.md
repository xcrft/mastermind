---
name: mastermind-structured-report-contract
description: Emit and validate Mastermind structured report tails for executor and auditor outputs. Triggers when producing or consuming execution reports, audit reports, defect lists, sentinels, or machine-readable workflow status.
metadata:
  version: 0.2.0
  authors:
    - mastermind
  tags:
    - workflow
    - reporting
---

# Structured report contract

The machine-readable handoff between executor → planner → auditor. Every executor and auditor reply ends with a fenced-YAML "tail" wrapped in HTML-comment sentinels, so the planner extracts it with one regex.

**Absence of the sentinel block is a malformed response.** A reply with prose but no tail cannot be consumed — the planner treats the turn as failed and re-spawns. The tail is the contract, not decoration.

The defect `kind:` field is a closed vocabulary — pick a listed kind from [`defect-taxonomy.md`](../mastermind-task-planning/references/defect-taxonomy.md) or use `kind: unclassified`. Full field-by-field meanings live in [`structured-report-schema.md`](../mastermind-task-planning/references/structured-report-schema.md).

## Executor tail

After the prose sections, emit:

````markdown
<!-- mastermind:report-begin -->
```yaml
schema_version: 1
spec: .mastermind/tasks/<NNN>-<name>/spec.md
status: complete | partial | failed
phases:
  - id: "1.1"
    status: done          # done | pending | stopped_here | skipped
files_modified:
  - path/relative/to/repo/root
claims:
  - kind: function_added
    symbol: <new symbol>
    file: path/relative/to/repo/root
    signature: "<exact indexed signature>"
  - kind: integration
    from: <changed caller>
    from_file: path/to/caller
    to: <existing callee>
    to_file: path/to/callee
    relation: calls
defects:
  - kind: <from defect-taxonomy.md, or unclassified>
    phase: "2.4"
    details: |
      <verbatim what went wrong>
    remediation_hint: |
      <a fix the planner can apply>
verifications:
  - cmd: "<command run>"
    result: pass | fail
    output_excerpt: "<~5 lines on fail>"
    observed:
      exit_code: 0
      tests_run: 12
```
<!-- mastermind:report-end -->
````

- `status`: `complete` (all phases landed, every final VERIFY exited 0) · `partial` (stopped mid-spec) · `failed` (Phase 1 couldn't start).
- `schema_version` is always `1`. The machine-readable source of truth is
  `schemas/executor-report-v1.schema.json`; unknown fields and unsupported
  versions fail closed. File-backed reports larger than 1 MiB are rejected
  before YAML decoding.
- `claims` records only concrete new-symbol and integration claims that the
  deterministic auditor can verify. Emit `claims: []` when neither applies;
  never translate general prose into a fabricated claim.
- `defects: []` + `status: complete` → planner proceeds to the auditor. Any defect → planner applies the taxonomy fix and re-spawns.
- `files_modified` must match `git diff --name-only HEAD` + untracked new files — it's the auditor's scope-creep anchor.

## Auditor tail

````markdown
<!-- mastermind:audit-begin -->
```yaml
spec: .mastermind/tasks/<NNN>-<name>/spec.md
verdict: held | drift | broken
scope_match: true
discrepancies:
  - kind: <from defect-taxonomy.md>
    symbol: <name>
    spec_says: <value>
    index_says: <value>
    evidence: "<what the diff / index showed>"
verifications_rerun:
  - cmd: "<command>"
    result: pass | fail
```
<!-- mastermind:audit-end -->
````

- `verdict`: `held` (every claim survived independent verification) · `drift` (≥1 non-critical discrepancy) · `broken` (≥1 critical: unexplained scope creep, verify fails on re-run, signature drift vs the spec's stated invariants).

## Status shapes at a glance

- **Complete** — `status: complete`, `defects: []`, every `verifications[].result: pass`.
- **Failed** — `status: failed`, one `defects[]` at `phase: "1.1"` with a `remediation_hint`, the blocking verification `result: fail`.
- **Partial** — `status: partial`, earlier phases `done`, the stopping phase `stopped_here` with its matching defect.

## Related skills

- [[mastermind-codegraph-research]] — ground structural claims in mmcg, not memory
- [[mastermind-investigation-ledger]] — diagnose an unknown bug before drafting a spec
