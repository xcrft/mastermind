---
name: mastermind-test-impact
description: Build a focused, evidence-backed test plan from `mastermind impact` or `mmcg_test_impact`. Use when deciding which tests to run for a change, explaining direct/transitive/heuristic candidates, or sequencing fast feedback before the repository's required full test gate.
metadata:
  version: 0.1.0
  authors: [mastermind]
  tags: [workflow, testing, mmcg]
---

# Mastermind Test Impact

Use deterministic candidates to prioritize feedback. Never treat them as proof
that omitted tests are unnecessary.

## Workflow

1. Choose the same trusted baseline and fresh index used for change analysis.
   Run `mastermind status` first. Stop on `index_stale` unless re-indexing is
   authorized; never infer candidates from a stale graph.
2. Run `mastermind impact --since REF --format json`, or call
   `mmcg_test_impact` for the exact test projection.
3. Reject a response that omits the
   `focused_tests_do_not_replace_full_gate` caveat.
4. Group candidates in this order:

   - `direct`: a changed test symbol at depth 0 or a graph-linked test at depth 1;
   - `transitive`: a graph-linked test at greater depth;
   - `heuristic`: a filename/component candidate without a proven call path.

5. For each test, retain file, symbol, line, depth, confidence, changed-symbol
   seeds, and structured evidence. Explain collisions or weak language edges.
6. Run focused tests from high to low confidence for fast feedback, then run
   every project-required phase/final gate.

Do not invent framework commands. Discover them from repository configuration
and the task spec. Do not call a heuristic candidate “covered”. If graph and
heuristic work were skipped with `work_limit`, report that no classified
candidates are available. A separately labeled fallback may widen by changed
modules and repository-owned test commands, but it is not `direct`,
`transitive`, or `heuristic` response evidence. Always retain the full gate.
