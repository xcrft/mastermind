---
name: mastermind-task-planning
description: Choose the lightest Mastermind workflow that fits the risk, then create an evidence-grounded verified or strict task contract for delegated implementation. Direct work deliberately uses no task spec.
metadata:
  version: 0.15.0
  authors: [mastermind]
  tags: [workflow, planning, delegation, mmcg, audit]
---

# Mastermind task planning

Plan only when a durable implementation contract adds value. The product has
three modes; ceremony is a risk control, not a default.

## Choose the mode first

| Mode | Use when | Contract |
|---|---|---|
| **direct** | Small, reversible work with a clear request | No task spec. Use map/impact/test-impact as needed, implement, run repository checks. |
| **verified** | Normal multi-file feature/fix or delegated work | Compact Goals, Scope, Acceptance Criteria, affected symbols, Tests Plan, Final Verification. |
| **strict** | Auth, billing, migration, public API, data-loss, supply-chain, or hard rollback | Verified contract plus alternatives, risk/evidence ledger, rollback, critic and security review where relevant. |

Do not create a fake direct-mode spec. If direct is appropriate, leave planner
mode and let the implementation agent work normally. `lite` and `standard` are
legacy task-file modes; do not create new ones.

## Planner boundary

- Research and decide; do not implement the spec yourself.
- Do not spawn an executor until the user approves the scope.
- State load-bearing assumptions. Ask only when choosing silently could change
  the delivered behavior or permission boundary.
- Keep unrelated cleanup out of Scope.

## Ground the contract

Use [[mastermind-codegraph-research]] for structural claims:

1. `mmcg_search` for every existing symbol named by the contract.
2. `mmcg_callers` / `mmcg_impact` for the symbols being changed.
3. `mmcg_change_impact` and [[mastermind-test-impact]] when a worktree already exists.
4. `mmcg_tasks` and `.mastermind/tasks/_lessons.md` only when prior work is relevant.

The graph is syntactic evidence, not runtime proof. Preserve collision,
precision, stale-index, and truncation notes. For one or two lookups work
inline; use the researcher for a bounded batch and the investigator only for an
unknown-cause bug.

## Design review

- **direct:** no critic.
- **verified:** use one [[mastermind-critical-review]] only when there is a real
  design fork, compatibility risk, or rollback concern.
- **strict:** independent critic is mandatory. Use three lenses only when the
  dimensions are genuinely independent, not automatically.
- Spawn the security auditor when the contract touches auth, secrets,
  permissions, tools, untrusted prompts, delegation boundaries, or supply chain.

Send reviewers a compact packet: problem, proposed design, concrete codegraph
evidence, constraints, and only plausible alternatives. Do not paste the whole
brainstorming transcript.

## Create a verified contract

Start from the CLI template:

```bash
mastermind new-spec "<description>" --mode verified
```

Fill only these sections:

- **Goals** — observable definition of done.
- **Scope** — owned files/components and explicit boundary.
- **Acceptance Criteria** — behavior that code or tests can demonstrate.
- **Pre-edit Snapshot** — only symbols actually changed; caller count and signature.
- **Implementation Plan** — outcome-oriented steps. Use literal FIND/CHANGE
  blocks only for a truly mechanical replacement.
- **Tests Plan** — which criterion each test proves.
- **Final Verification** — focused tests plus the repository-required gate.
- **Notes** — only material assumptions, alternatives, docs, observability, or performance impact.

For strict work use `--mode strict` and retain the additional risk, evidence,
rollback, and critic sections. Delete placeholders; never pad a section with
generic engineering advice.

## Pre-flight

Before showing the contract to the user:

1. Every scoped path exists or is explicitly marked new.
2. Existing symbols and snapshot counts match the current index.
3. Acceptance Criteria are independently observable.
4. VERIFY commands are real, terminating, and scoped; the full gate appears
   once in Final Verification.
5. The contract authorizes every intended file and no unrelated file.

Run the deterministic gate:

```bash
mastermind run-task <task>/spec.md --pre-only
```

Fix failures before requesting approval. A verified contract should normally
fit on one or two screens; strict contracts may be longer because their extra
evidence is material.

## Execution handoff

After approval, invoke [[mastermind-task-executor]] with the spec path. The
executor writes `<task>/executor-report.md` containing the prose report and the
canonical schema-v1 tail from [[mastermind-structured-report-contract]]. It
must not write lifecycle state.

Route malformed or partial reports by their typed defect kind. Stop after three
failed execution cycles and return to design rather than looping.

## Post-flight

Run the controller-owned deterministic audit:

```bash
mastermind run-task <task>/spec.md --post-only
```

Post-flight requires `executor-report.md`, checks its claims against the live
index and diff, writes `audit.md`, and updates the task-local `state.json`.

- **verified:** deterministic audit plus planner semantic review is sufficient
  when the verdict is held and no high-risk uncertainty remains.
- **strict:** spawn `mastermind-auditor` for an independent read-only review
  before completion.
- Any drift/broken verdict returns to the planner; do not present it as done.

### Step 9a — Mechanical audit

Use the controller result. For strict work, compare it with the independent
auditor's structured tail.

### Step 9b — Semantic review

Check whether the implemented behavior solves the original request, whether
deferred items are acceptable, and whether tests demonstrate the acceptance
criteria. Mechanical contract compliance does not answer product judgment.

### Step 9c — Persist the reviewed result (planner/controller only)

The auditor is repository-read-only and must not mutate evidence. `run-task`
owns `state.json`, `audit.md`, lessons, and release-note eligibility. For a
manual strict audit, persist the planner-persisted auditor verdict only after
parsing it and completing semantic review.

### Step 9d — Report

Tell the user: outcome, material changes, verification actually run, audit
verdict, and any unresolved limitation. Update `CONTEXT.md` only for durable
project knowledge; never add a ceremonial “nothing changed” entry.

Commit, push, PR, release, and publication remain separate actions requiring
explicit user authorization.
