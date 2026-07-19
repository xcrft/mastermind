---
name: mastermind-architecture-review
description: Review a proposed or existing architecture against the real runtime path, source-of-truth ownership, retry and idempotency behavior, and backward-compatibility constraints. Use for service boundaries, async workflows, persistence changes, external integrations, migrations, public APIs, events, schemas, or designs whose component diagram looks plausible but runtime safety is not yet proven.
metadata:
  version: 0.2.0
  authors: [mastermind]
  tags: [workflow, architecture, review, reliability, contracts]
---

# Mastermind Architecture Review

Review whether a design preserves the system's runtime invariants. Do not turn
the review into a generic architecture checklist or redesign the system merely
because another style is possible.

## Boundaries

- [[mastermind-project-map]] supplies bounded structural navigation; it does
  not prove a request, event, or write reaches production code at runtime.
- [[mastermind-change-impact]] supplies syntactic blast-radius evidence; it
  does not prove semantic compatibility.
- [[mastermind-critical-review]] is the general proposal rubric. Use this skill
  when the decision specifically crosses runtime, state, retry, or evolution
  boundaries.

This is a read-only review. It may require a design change, but it does not
implement one.

## Inputs

- **Decision** — the architecture or change being reviewed.
- **Scope** — affected components, interfaces, state, and deployment boundary.
- **Evidence** — codegraph results, entry points, handlers, schemas, storage
  code, tests, configs, deployment facts, or an explicit evidence gap.
- **Baseline** — current contract or behavior when compatibility is relevant.

Never replace missing evidence with a familiar architecture pattern.

## Review workflow

1. State the decision and the system invariant it must preserve.
2. Gather the narrowest evidence that can prove the actual path: indexed
   structure first, then read the entry point, boundary adapter, domain logic,
   state owner, and externally visible contract. Treat dynamic dispatch,
   framework registration, queues, reflection, and infrastructure routing as
   runtime evidence gaps until verified directly.
3. Reconstruct the path as ordered hops:

   ```text
   trigger -> transport -> admission/auth -> domain operation -> state owner
           -> external side effect -> response/event
   ```

   Omit hops that do not exist; never add conventional layers by assumption.
4. Load only the references implicated by the design:
   - process, service, queue, trust, or serialization crossings:
     [`references/runtime-boundaries.md`](references/runtime-boundaries.md)
   - authoritative and derived state, caches, indexes, replicas, or dual writes:
     [`references/source-of-truth.md`](references/source-of-truth.md)
   - retries, webhooks, commands, jobs, payments, or at-least-once delivery:
     [`references/idempotency.md`](references/idempotency.md)
   - API, event, schema, config, CLI, persisted-data, or rolling-deploy changes:
     [`references/backward-compatibility.md`](references/backward-compatibility.md)
5. For each material risk, describe one concrete failure sequence. Name the
   invariant at risk, the evidence, and the boundary where it can fail.
6. Bind every required change to a verification method that would fail before
   the change: a contract test, integration test, concurrent retry test,
   replay test, migration rehearsal, or production observation.
7. Give a bounded verdict. Unknown runtime facts stay unknown.

## Evidence rules

- An import edge or directory boundary is discovery evidence, not a runtime
  call-path proof.
- A cache, search index, replica, or materialized view is not authoritative
  merely because the reviewed handler reads it.
- An HTTP method, idempotency key field, or queue deduplication setting does not
  prove idempotency without operation scope, durable ownership, and atomicity.
- An additive schema diff is not automatically compatible; old readers,
  stored messages, defaults, enum handling, and rollout order still matter.
- If evidence cannot distinguish safe from unsafe, use `insufficient evidence`
  and name the exact file, contract, or runtime observation needed.

## Severity and verdict

- **P0** — credible data loss, security breach, or duplicate money movement.
- **P1** — broken runtime path, split authority, duplicate side effect, or
  incompatible deployed contract.
- **P2** — missing proof, unsafe rollout assumption, weak recovery, or an
  untested boundary.
- **P3** — clarity or maintainability issue without a demonstrated contract risk.

Verdict is exactly one of: `sound`, `sound with constraints`, `revise`, or
`insufficient evidence`.

## Output contract

```markdown
## Architecture review

**Verdict:** sound | sound with constraints | revise | insufficient evidence
**Decision:** <one sentence>
**Invariant:** <what must remain true>
**Evidence scope:** <files, contracts, runtime observations, and gaps>

### Runtime path
| Hop | Boundary/owner | Evidence | Contract or unknown |
|---|---|---|---|

### Findings
| Severity | Invariant at risk | Evidence | Failure sequence | Required change |
|---|---|---|---|---|

### Verification
| Risk | Proof required |
|---|---|

### Unknowns
- <only decision-changing missing evidence>

### Epistemic envelope
- **Observed:** <direct source, graph, test, or runtime evidence>
- **Inferred:** <bounded conclusion and reasoning>
- **Confidence:** high | medium | low — <reason>
- **Would change this conclusion:** <specific falsifier or superseding evidence>
```

Use at most seven findings. Do not emit generic advice, a technology shopping
list, or speculative scalability work unrelated to the decision.
