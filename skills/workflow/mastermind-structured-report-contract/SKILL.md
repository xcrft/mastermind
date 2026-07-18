---
name: mastermind-structured-report-contract
description: Produce or consume the canonical file-backed Mastermind executor report parsed by `run-task --post-only`. Use for executor evidence, schema-v1 report validation, controller audit inputs, or strict auditor advisory output.
metadata:
  version: 0.3.0
  authors: [mastermind]
  tags: [workflow, reporting]
---

# Structured report contract

The only canonical machine-consumed agent handoff is
`<task>/executor-report.md`. The Rust controller reads that file, extracts its
sentinel-wrapped YAML, validates schema v1, and uses its supported claims and
verification observations during post-flight.

A copy returned in chat is useful to humans but is not the lifecycle record.
The controller does not scrape planner chat, automatically patch a spec, or
re-spawn an executor. Missing or malformed file evidence fails post-flight.

## Executor tail

Append this tail to the prose report in `executor-report.md`:

````markdown
<!-- mastermind:report-begin -->
```yaml
schema_version: 1
spec: .mastermind/tasks/<NNN>-<name>/spec.md
status: complete | partial | failed
phases:
  - id: plan-1
    status: done          # done | pending | stopped_here | skipped
files_modified:
  - path/relative/to/repository
claims:
  - kind: function_added
    symbol: <new symbol>
    file: path/to/file
    signature: "<indexed signature when available>"
  - kind: integration
    from: <changed caller>
    from_file: path/to/caller
    to: <existing callee>
    to_file: path/to/callee
    relation: calls
defects:
  - kind: <recommended label or unclassified>
    phase: plan-2
    details: <concrete failure evidence>
    remediation_hint: <bounded next action>
verifications:
  - cmd: "<command actually run>"
    result: pass | fail
    output_excerpt: "<short excerpt on failure>"
    observed:
      exit_code: 0
      tests_run: 12
```
<!-- mastermind:report-end -->
````

`phases` is the schema-v1 compatibility name for execution steps. New specs may
use IDs such as `plan-1`; they do not need phase-shaped prose or checklists.

The machine source of truth is
`schemas/executor-report-v1.schema.json` plus the stricter Rust consistency
checks. Unknown fields, unsupported versions, duplicate step IDs, empty
commands, contradictory complete reports, and reports over 1 MiB fail closed.

## Field rules

- `complete`: every reported step is `done`, `defects` is empty, and every
  verification result is `pass`.
- `partial` or `failed`: at least one concrete defect is required.
- `files_modified`: the executor's evidence, not the scope authority. The
  controller calculates the real changed-file set from git and compares it with
  the spec.
- `claims`: only `function_added` and `integration` are understood by the
  deterministic audit. Use `claims: []` for general prose outcomes.
- `defects[].kind`: a recommended routing label from `defect-taxonomy.md`, not a
  closed enum and not an instruction for automatic repair.
- `verifications`: record commands actually run. Do not claim an inferred or
  intended check.

## Independent auditor output

Strict mode may ask a read-only auditor for a sentinel-wrapped advisory tail:

````markdown
<!-- mastermind:audit-begin -->
```yaml
spec: .mastermind/tasks/<NNN>-<name>/spec.md
verdict: held | drift | broken
scope_match: true
discrepancies: []
verifications_rerun: []
```
<!-- mastermind:audit-end -->
````

This tail is currently a human/planner review contract, not a Rust-parsed
lifecycle input. `audit.md` and `state.json` remain controller-owned. Do not
claim that an auditor chat message was persisted unless a future controller
command actually implements that transition.

## Routing

The planner reads malformed, partial, or failed evidence and decides whether to
repair implementation, revise the contract, or stop. Recommended defect labels
help grouping; they do not authorize deterministic auto-fixes. After three
cycles blocked by the same condition, return to design or the user.

## Related skills

- [[mastermind-task-executor]] — writes the canonical file.
- [[mastermind-task-planning]] — owns contract and semantic review.
- [[mastermind-codegraph-research]] — grounds structural claims.
