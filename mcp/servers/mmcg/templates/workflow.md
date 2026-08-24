---
name: mastermind-workflow
description: Compact project router for the Mastermind Direct, Verified, and Strict workflows.
metadata:
  version: 2.0.1
  authors: [mastermind]
  tags: [claude-md, workflow, delegation, audit]
---

# <PROJECT_NAME>

<What this project is and is not.>

## Orientation

- **Stack:** <language/framework>
- **Entry point:** `<path>`
- **Core code:** `<path>`
- **Project knowledge:** [CONTEXT.md](CONTEXT.md)

## Commands

```bash
# focused tests
<FOCUSED_TEST_COMMAND>

# repository-required gate
<FULL_GATE_COMMAND>
```

## Mastermind workflow

Use the lightest mode that fits the risk:

| Mode | Use | Flow |
|---|---|---|
| **Direct** | Small, reversible, clear work | inspect → impact if useful → implement → tests → comment-delta gate |
| **Verified** | Normal multi-file or delegated work | compact task contract → deterministic pre/post gates → semantic review |
| **Strict** | Auth, billing, migration, public API, data loss, supply chain | verified flow + critic/security/rollback evidence + independent auditor |

Do not create a spec for Direct work.

Direct work has no controller and no post-flight, so the only review it gets is
the one you run. Once the change is finished, inspect `git diff -U0 <baseline>`
and untracked files for added, modified, or deleted comments. Spawn
`mastermind-comment-auditor` only when that comment delta is non-empty.

### Grounding

Use the mmcg MCP tools before making structural claims:

- `mmcg_search` for symbol existence;
- `mmcg_callers` / `mmcg_impact` for blast radius;
- `mmcg_change_impact` and `mmcg_test_impact` for an existing diff;
- `mmcg_map` for unfamiliar architecture;
- `mmcg_callers` on a component for who renders it — JSX and Vue template usage
  are call edges.

The graph is syntactic and bounded. Preserve stale-index, collision, precision,
and truncation caveats; source reads and tests remain authoritative for runtime behavior.

### Verified and Strict tasks

1. Use `mastermind-task-planning` to create and ground the contract.
2. Run the read-only contract validator: `mastermind verify-spec <spec>`.
3. The user approves Scope and Acceptance Criteria.
4. Run the state-writing pre-flight:

   ```bash
   mastermind run-task .mastermind/tasks/<task>/spec.md --pre-only
   ```

5. Use `mastermind-task-executor`. It writes
   `<task>/executor-report.md` and never writes lifecycle state.
6. Run controller-owned post-flight:

   ```bash
   mastermind run-task .mastermind/tasks/<task>/spec.md --post-only
   ```

7. Inspect the post-flight baseline diff and untracked files for a comment
   delta. Spawn `mastermind-comment-auditor` only when at least one comment was
   added, modified, or deleted. Its findings are input to your semantic review,
   not a verdict.
8. Perform semantic review. Strict work additionally requires the read-only
   `mastermind-auditor`.

The controller is the only owner of `state.json`, `audit.md`, lessons, and
release eligibility. A missing or malformed executor report fails post-flight.

### Discipline routing

`mmcg_change_impact` reports a `disciplines` block derived from the changed
paths. Read it instead of classifying the request yourself, and load at most one
research skill and one audit skill per detected discipline:

| discipline | before the change | after the change |
|---|---|---|
| `frontend` | [[mastermind-component-research]] | [[mastermind-frontend-audit]] |
| `qa` | [[mastermind-test-impact]] | [[mastermind-test-audit]] |
| `migration` | [[mastermind-runtime-research]] | [[mastermind-architecture-review]] |

`unclassified` paths are not "no discipline" — they are paths whose discipline a
path cannot establish. A queue consumer, a migration, or an auth boundary in a
plain `.ts` file lands there. When you judge one to be service or state work, the
pair is [[mastermind-runtime-research]] before and
[[mastermind-architecture-review]] after; that call is yours, not the
classifier's. The block proposes an evidence set; it never locks one.

A detected `migration` also raises the mode: the table above already lists
migrations as Strict work, so a SQL file or a migrations directory in the diff
is a mode trigger the paths establish rather than a judgement call. It says
nothing about what the migration does — destructive, backfilling, or additive is
a question for the review.

Pre-flight, before a diff exists, route on the paths named in the spec's Scope.

### Role routing

- Researcher: bounded batches of repository facts.
- Investigator: bugs whose cause is not known.
- Critic: real design forks; mandatory for Strict.
- Security research: what reaches the privileged operation, what statically
  applies the guard, who reads secrets. The graph is wrong in both directions —
  a missing edge is not absence of access, a present edge is not enforcement.
- Security auditor: permissions, secrets, tools, untrusted input, delegation,
  supply chain, or audit policy.
- Executor: implementation within approved scope.
- Auditor: independent read-only review for Strict work.
- Comment auditor: non-empty comment delta of a finished change, in every mode;
  skip the spawn when the gate finds no changed comments.
- Runtime research: who already consumes a service, who writes the state, which
  boundaries the change crosses — and which invocations the graph cannot see at
  all. Zero static callers on a handler is a gap, not an absence.
- Test auditor: does the change's behaviour have a `direct` test that reaches
  the production path — a `heuristic` candidate is a filename match, not
  coverage.
- Frontend auditor: React or Vue change — unrendered components, props-contract
  breaks, duplicates, raw values. Research the component graph before writing
  UI, not after.
- Design intake: a design handoff becomes named components, token names, and
  criteria that can fail. Visual fidelity is parked explicitly, never smuggled
  into acceptance criteria.
- Product intake: a PRD or ticket becomes behaviour, constraint, and outcome.
  Only the first is acceptance criteria; a success metric cannot fail at merge
  time, and the cases the PRD never mentions are asked before approval.
- Browser verification: record what was observed and at which viewport. The
  accessibility tree and the console are evidence; a screenshot is not, and an
  unchecked item is marked unchecked.

Skip roles that add no new evidence.

### Completion and release

Report only checks actually run and limitations still open. A Held mechanical
contract is not a substitute for semantic judgment.

Commit, push, PR, tag, release, and publication require explicit user
authorization. Never infer release authority from implementation approval.
