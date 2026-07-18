---
name: mastermind-workflow
description: Compact project router for the Mastermind Direct, Verified, and Strict workflows.
metadata:
  version: 1.0.0
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
| **Direct** | Small, reversible, clear work | inspect → impact if useful → implement → tests |
| **Verified** | Normal multi-file or delegated work | compact task contract → deterministic pre/post gates → semantic review |
| **Strict** | Auth, billing, migration, public API, data loss, supply chain | verified flow + critic/security/rollback evidence + independent auditor |

Do not create a spec for Direct work.

### Grounding

Use the mmcg MCP tools before making structural claims:

- `mmcg_search` for symbol existence;
- `mmcg_callers` / `mmcg_impact` for blast radius;
- `mmcg_change_impact` and `mmcg_test_impact` for an existing diff;
- `mmcg_map` for unfamiliar architecture.

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

7. Perform semantic review. Strict work additionally requires the read-only
   `mastermind-auditor`.

The controller is the only owner of `state.json`, `audit.md`, lessons, and
release eligibility. A missing or malformed executor report fails post-flight.

### Role routing

- Researcher: bounded batches of repository facts.
- Investigator: bugs whose cause is not known.
- Critic: real design forks; mandatory for Strict.
- Security auditor: permissions, secrets, tools, untrusted input, delegation,
  supply chain, or audit policy.
- Executor: implementation within approved scope.
- Auditor: independent read-only review for Strict work.

Skip roles that add no new evidence.

### Completion and release

Report only checks actually run and limitations still open. A Held mechanical
contract is not a substitute for semantic judgment.

Commit, push, PR, tag, release, and publication require explicit user
authorization. Never infer release authority from implementation approval.
