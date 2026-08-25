---
name: mastermind-task-executor
description: Execute an approved Mastermind task contract within Scope, prove its Acceptance Criteria, and write the canonical file-backed executor report. Use when the user hands off a `.mastermind/tasks/<NNN>-<name>/spec.md` or explicitly asks to execute an approved Mastermind task.
workflow:
  schema_version: 1
  activation: manual
  mutability: writer
  writes:
    - artifact: task.executor-report
      path: ".mastermind/tasks/{task}/executor-report.md"
      authority: canonical
      runtime: portable
      exclusivity_group: task-executor
metadata:
  version: 0.6.2
  authors: [mastermind]
  tags: [workflow, execution, delegation, mmcg]
---

# Mastermind task executor

Implement the approved outcomes in `spec.md`; do not reinterpret the product
request or widen Scope. Acceptance Criteria define success. Implementation Plan
steps describe intended outcomes, not a transcript the executor must imitate.
Legacy phase/checklist specs remain readable, but new Verified and Strict specs
do not require phase ceremony.

## Activation boundary

Use this skill only when the caller provides an approved task path or an
approved spec. Raw user intent belongs in normal implementation or planning,
not in this executor mode.

The task folder may contain reports or scratch files. `spec.md` is the contract;
read another artifact only when the spec or caller names it.

## Before editing

1. Read the complete spec.
2. Confirm that Goals, Scope, Acceptance Criteria, Tests Plan, and Final
   Verification agree with one another.
3. Confirm every intended edit is authorized by `touches` or Scope.
4. When `~/.mastermind/style.md` exists, use relevant non-conflicting rules as
   preferences. Repository code, formatter/linter configuration, and the spec
   take precedence over the user-global profile. Deterministic code-shape
   observations are diagnostic evidence, not implementation instructions; do
   not apply a language-specific observation to another language. Commit voice
   is fallback-only when repository policy is silent.
5. For a named symbol, use mmcg to check its current location and impact. Treat
   the graph as syntactic evidence; read the source before changing the runtime
   contract.

Stop before editing when the spec is contradictory, an authorized path is
missing, the structural evidence is materially stale, or the requested work
would cross Scope, security, compatibility, or permission boundaries.

## Implement and verify

For each Implementation Plan step, or each legacy phase:

1. Implement the stated outcome inside Scope.
2. Apply literal `FIND:` / `CHANGE TO:` blocks exactly when present. A mismatch
   is contract drift; do not fuzzy-match it.
3. Otherwise follow Acceptance Criteria and surrounding project conventions.
4. Run the focused verification associated with the changed behavior.

Classify a failure before deciding whether to continue:

| Failure | Response |
|---|---|
| Contract contradiction, missing prerequisite, unsafe scope expansion, stale required evidence | Stop and return to the planner. |
| Environment failure unrelated to the edit | Re-check once; then stop with the exact blocker. |
| Test or build failure caused by the in-scope implementation | Fix the implementation and rerun the focused check. |
| Literal FIND mismatch | Stop; report expected and actual text. |

Use a bounded repair loop: at most three implementation-and-check attempts for
the same failing condition. If it still fails, report `partial` or `failed` with
the evidence. Do not hide a failure by weakening a test, removing an acceptance
criterion, or changing the spec.

Run every command in Final Verification after the focused checks pass. Commands
must terminate; do not launch a server or watcher as verification.

## Comments

Before Final Verification, inspect comments added or modified by this task.
Default to zero new comments. Delete narration of code, section banners, step
labels, edit markers, signature echoes, dead code, and ownerless TODOs. Keep
only required documentation, licenses, invariants, security constraints, and
non-obvious reasons that would be lost if the comment were removed. Do not
clean unrelated comments merely because a file was opened. This is the
[[no-ai-slop-comments]] gate, not an optional style suggestion.

## Canonical report

Write the complete report to `<task>/executor-report.md`; returning the same
text in chat is optional convenience. Never write `state.json` or `audit.md`.
The `mastermind run-task --post-only` controller owns lifecycle and audit state.

Use plan-step IDs such as `plan-1`, `plan-2`; for a legacy spec, its existing
phase IDs are also valid:

````markdown
## Task <XXX> — execution report

**Spec:** `.mastermind/tasks/<NNN>-<name>/spec.md`
**Status:** complete | partial | failed

### Outcomes completed
- `plan-1` — <observable outcome>

### Verification results
- `<command>` → passed | failed: <short exact evidence>

### Files modified
- `path/relative/to/repository`

### Deferred or blocked
<Out-of-scope observations or the exact blocker; omit when empty.>

<!-- mastermind:report-begin -->
```yaml
schema_version: 1
spec: .mastermind/tasks/<NNN>-<name>/spec.md
status: complete
phases:
  - id: plan-1
    status: done
files_modified:
  - path/relative/to/repository
claims: []
defects: []
verifications:
  - cmd: "<command actually run>"
    result: pass
    observed:
      exit_code: 0
```
<!-- mastermind:report-end -->
````

The field is named `phases` for schema-v1 compatibility; its IDs represent
actual plan steps and do not require a phase-shaped spec. Use only the claim
types supported by [[mastermind-structured-report-contract]]. Defect kinds are
recommended routing labels, not permission to invent an automatic repair.

Complete means every acceptance criterion was demonstrated, every Final
Verification command passed, all reported steps are `done`, and `defects` is
empty. Partial/failed reports must name at least one concrete defect. The Rust
parser rejects contradictory shapes.

## Decision boundary

- A better local implementation that preserves the approved outcomes is normal
  executor judgment.
- A change to observable behavior, Scope, permissions, migration strategy,
  public API, or an acceptance criterion requires planner/user review.
- Unrelated bugs belong in `Deferred or blocked`; do not fix them here.

## Related skills

- [[mastermind-task-planning]] — creates the approved contract.
- [[mastermind-structured-report-contract]] — defines the file-backed schema.
- [[mastermind-codegraph-research]] — grounds structural discovery.
