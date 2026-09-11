---
name: mastermind-project-history
description: Retrieve and reason from durable project decisions, failed approaches, audits, reports, and lessons without treating provenance, search rank, or user approval as technical proof. Use when asking why a design exists, whether an approach was tried, what supersedes an older decision, what prior evidence should constrain a new plan, or how to record and check explicit document-to-code evidence links.
metadata:
  version: 0.3.0
  authors: [mastermind]
  tags: [workflow, history, decisions, provenance, evidence]
---

# Mastermind Project History

Recover decision context without inventing institutional memory. Markdown
artifacts are authoritative; `mmcg_history` is a rebuildable retrieval index.

## Sources

The history corpus admits only:

- `CONTEXT.md`
- root-level `CONTEXT-archive-*.md`
- `.mastermind/tasks/<task>/spec.md`
- `.mastermind/tasks/<task>/executor-report.md`
- `.mastermind/tasks/<task>/audit.md`
- `.mastermind/releases/<name>.md`
- legacy `.mastermind/tasks/<task>/release-notes.md`
- `.mastermind/tasks/_lessons.md`
- Markdown files, including nested directories, under `docs/adr/`,
  `docs/adrs/`, `docs/decisions/`, `adr/`, `adrs/`, or
  `.mastermind/decisions/` (`kind: architecture_decision`)

Arbitrary scratch files are not history. Git history and current runtime code
may contradict or supersede a record, so inspect them when the answer is
load-bearing.

## Workflow

1. Query `mmcg_history` with the narrowest useful terms and optional `kind`.
2. Read the returned Markdown around each relevant match. Search rank is not
   confidence, and co-occurrence is not causality.
3. Resolve explicit status and supersession links. An accepted or active record
   can constrain the plan; a proposed record remains a proposal. Follow
   `Supersedes` links in both directions and inspect the replacement's status.
   A newer date or filename alone does not replace an accepted decision.
   If status is absent or records conflict, say so.
   A lesson with status `candidate` is an audit signal awaiting semantic review,
   not active guidance and not proof of a reusable root cause.
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

## Reusing document evidence

When a research packet needs reusable links between a decision and current
code, use the bundled [document graph helper](scripts/document_graph.py).
Declare only relations supported by a source passage and cite both endpoints.
The helper records the declared paths, lines, and content hashes in a new local
artifact under `.mastermind/research/`; it does not edit history or the codegraph.
See [the commands and contract](references/document-evidence-graph.md).

Before reusing a snapshot, run `check`. A changed or missing endpoint makes
every incident edge `needs_review`; read the changed source before repeating
the conclusion. Unaffected edges can remain `current`. When the research depends
on a collection of decisions, declare its directories with repeatable
`snapshot --corpus-dir docs/adr` options. This also captures non-hidden Markdown
documents recursively within those directories, including uncited documents.
Choose a small relevant scope; do not assume the entire repository was searched.

Read the separate `corpus.status` before reusing a conclusion. `changed` makes
the overall check `needs_review`, even when every edge remains `current`.
Inspect the added, missing, or changed documents and search history again for
superseding records and contradictions. `not_tracked` means no corpus inventory
was saved, including in legacy snapshots. `current` covers only the chosen
directories and cannot prove completeness outside them.

`current` means the named files still match the snapshot. Every edge remains
`unverified`, including `verified_by`: a hash or relation name cannot establish
that code obeys a decision or that a test passed. Review semantic claims and
runtime evidence separately. A `mentions` edge is only a mention.

Without corpus tracking, the graph covers only named files. A newly added
superseding ADR is not detected by endpoint hashes: search history again before
making corpus-wide or current policy claims. `revision_changed` is separate
from endpoint and corpus freshness; even an unchanged revision does not prove
a clean worktree or complete evidence. A corpus read, traversal, or limit error
is incomplete evidence; resolve it before reusing the snapshot.

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
