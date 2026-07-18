---
name: mastermind-change-impact
description: Produce an evidence-backed change-impact brief with `mastermind impact` or `mmcg_change_impact`. Use before editing, during implementation planning, or in PR review to trace changed symbols, callers, component/API crossings, collision risk, and likely affected consumers since a git ref.
metadata:
  version: 0.1.0
  authors: [mastermind]
  tags: [workflow, mmcg, impact]
---

# Mastermind Change Impact

Base the brief on the deterministic change-impact response, not a grep-derived
caller list.

## Workflow

1. Select an explicit trusted baseline such as the merge base or target branch.
   Do not silently substitute `HEAD~1`.
2. Run `mastermind status` as a read-only preflight. Ensure the current working
   tree is indexed when local changes are in scope. If the backend returns
   `index_stale`, stop unless the request authorizes `mastermind index .`; do
   not downgrade confidence and continue with a partial graph.
3. Run:

   ```bash
   mastermind impact --since REF --format json --depth 3 --top 100
   ```

   Use `mmcg_change_impact` through MCP when appropriate.
4. Verify full baseline/head object IDs and whether worktree/untracked files are
   included. Check every limit and precision note before ranking risk.
5. Report changed symbols, direct then transitive consumers, component/API
   crossings, and public-surface implications. Attach each conclusion to the
   response's changed-symbol seeds and repository-relative evidence.

## Confidence rules

Downgrade confidence when names collide, edge precision is weak, the index is
stale, a work limit skipped expansion, or dynamic/reflection boundaries are
likely. Removed symbols can still have old-side evidence; do not require them
to exist in the live index.

Do not convert syntactic impact into a semantic guarantee. Do not say an
unlisted consumer is safe. If the backend is unavailable, report the missing
capability rather than inventing an impact graph.

For a blocked result, state the trusted baseline, the stable failure code, the
required remediation, and that affected symbols/components/consumers are
unknown until a fresh deterministic response exists.
