# Task contract templates

Direct mode has no task file. Start normal work with codegraph evidence and
repository tests.

## Verified

Use `mastermind new-spec "<description>" --mode verified`; it generates the
canonical frontmatter and headings. Fill this compact contract:

````markdown
---
verify:
  - cmd: "<focused test>"
    run:
      id: focused
      argv: [<executable>, <argument>]
      cwd: .
      timeout_secs: 300
  - cmd: "<repository-required gate>"
    run:
      id: gate
      argv: [<executable>, <argument>]
      cwd: .
      timeout_secs: 300
acceptance:
  - id: outcome
    statement: "<observable behavior this task must achieve>"
    checks: [focused, gate]
---

# Task NNN: <title>

## Goals
- <observable definition of done>

## Scope
- Change: `<path>` — <intended outcome>
- Do not change: <boundary>

## Acceptance Criteria
- [ ] <behavior that can be asserted>

## Pre-edit Snapshot
- `<symbol>` — <caller count>; signature `<signature>`

## Implementation Plan
1. <outcome-oriented change>
2. <test or compatibility work>

## Tests Plan
- `<test>` — proves <criterion>

## Final Verification
```bash
<focused test>
<repository-required gate>
```

## Notes
- <only material assumptions, alternatives, docs, observability, or performance impact>
````

Use literal `FIND:` / `CHANGE TO:` blocks only when exact replacement is part
of the contract. Otherwise acceptance criteria define correctness.

Retain the generated scope frontmatter. Use `touches` for existing files and
`creates: [src/new.py, docs/new.md]` for additions. A new required document also
belongs in `expected_docs`. Existing drafts allow another preflight, but an old
baseline file cannot be relabeled as a creation. Mirror every required Final Verification
command in `verify[].cmd`, including its arguments. Legacy `VERIFY:` command
lines are also machine-checked; labels and ordinary shell fences are not.

The planner must replace the generated `acceptance` placeholders and map every
criterion to relevant `verify[].run.id` checks before preflight. Each command
label must match its displayed argv. All checks listed for a criterion are
required. Run them through `mastermind verification run <spec> --id <id>` after
implementation, then inspect `mastermind acceptance status <spec> --json`.
Markdown checkboxes and executor pass claims do not satisfy these requirements.
Review whether the chosen tests actually cover the criterion: a current passing
run establishes the declared evidence requirement, not semantic entailment.

## Strict additions

Start with `--mode strict` and retain the generated sections that are material:

- alternatives and decision rationale;
- risk/evidence ledger;
- rollback or migration boundary;
- design critic verdict;
- security review for auth, secrets, permissions, tool/agent boundaries, or
  supply-chain changes.

Do not pad strict sections with generic advice. Every claim must point to
codegraph, repository, test, or operational evidence.

## Ownership and lifecycle

- Planner owns `spec.md` and scope approval.
- Executor owns `<task>/executor-report.md` and never writes lifecycle state.
- `mastermind run-task` owns `<task>/state.json`, `audit.md`, lessons, and
  release-note eligibility.
- Post-flight requires the canonical report and compares it with the spec,
  index, and real diff.
