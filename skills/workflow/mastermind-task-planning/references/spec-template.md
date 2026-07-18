# Task contract templates

Direct mode has no task file. Start normal work with codegraph evidence and
repository tests.

## Verified

Use `mastermind new-spec "<description>" --mode verified`; it generates the
canonical frontmatter and headings. Fill this compact contract:

````markdown
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
