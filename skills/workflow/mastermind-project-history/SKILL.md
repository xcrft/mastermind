---
name: mastermind-project-history
description: Retrieve and reason from durable project decisions, failed approaches, audits, reports, and lessons without treating provenance, search rank, or user approval as technical proof. Use when asking why a design exists, whether an approach was tried, what supersedes an older decision, or what prior evidence should constrain a new plan.
metadata:
  version: 0.1.0
  authors: [mastermind]
  tags: [workflow, history, decisions, provenance, evidence]
---

# Mastermind Project History

Recover decision context without inventing institutional memory. Markdown
artifacts are authoritative; `mmcg_history` is a rebuildable retrieval index.

## Sources

The history corpus admits only:

- `CONTEXT.md`
- `.mastermind/tasks/<task>/spec.md`
- `.mastermind/tasks/<task>/executor-report.md`
- `.mastermind/tasks/<task>/audit.md`
- `.mastermind/tasks/<task>/release-notes.md`
- `.mastermind/tasks/_lessons.md`

Arbitrary scratch files are not history. Git history and current runtime code
may contradict or supersede a record, so inspect them when the answer is
load-bearing.

## Workflow

1. Query `mmcg_history` with the narrowest useful terms and optional `kind`.
2. Read the returned Markdown around each relevant match. Search rank is not
   confidence, and co-occurrence is not causality.
3. Resolve status and chronology. Prefer an `active` decision over a
   `superseded` one and follow `Supersedes` links. If status is absent, say so.
4. Preserve negative history: a relevant rejected alternative, failed attempt,
   audit defect, or gotcha must constrain the new plan unless new evidence
   directly addresses its failure mode.
5. Verify technical claims against current code, tests, or runtime evidence.
   Provenance answers "where did this claim come from?"; it does not answer
   "is this claim true?" User approval proves authorization, not correctness.
6. If evidence is thin for a security, runtime-boundary, migration, money,
   idempotency, or compatibility conclusion, return `insufficient evidence` and
   name the missing proof.

Do not write project history during retrieval. The planner/controller records
durable knowledge only after post-flight semantic review.

## Output contract

```markdown
## Project history

**Question:** <what is being explained>

### Observed
- <record, status, provenance, evidence, and path>

### Inferred
- <bounded explanation and why it follows from the observed records>

### Unknown
- <decision-changing missing evidence or `none material`>

### Confidence
**Level:** high | medium | low
**Reason:** <evidence quality and currency, not hit count>

### Would change this conclusion
- <superseding record, contradictory runtime fact, or verification result>

### Plan constraints
- <relevant dead end, invariant, or lesson the next plan must honor>
```

Never collapse Observed and Inferred into one confident narrative. When no
matching history exists, say "not found under this query" rather than "never
happened."

## Related skills

- [[mastermind-codegraph-research]] — verify current structural claims
- [[mastermind-architecture-review]] — review runtime and evolution invariants
- [[mastermind-task-planning]] — persist reviewed durable decisions post-flight
