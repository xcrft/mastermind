---
name: mastermind-critical-review
description: Stress-test a proposed design, task spec, implementation plan, or executor report for false assumptions, broken contracts, scope creep, missing evidence, and high-risk failure modes. Use before drafting sensitive specs, before approving a plan, or when a critic/auditor needs a compact review rubric.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - workflow
    - review
    - critique
    - planning
    - audit
---

# Mastermind Critical Review

Challenge a proposed design or completed change before it becomes accepted work. The goal is not to be negative — it's to prevent confident wrongness.

This is the rubric; the spawnable agent that applies it is the `mastermind-critic` subagent (Opus, independent context).

## When to use

- Before approving a non-trivial task spec.
- Before accepting a design that touches auth, billing, migrations, public APIs, data loss, or rollback complexity.
- When a planner asks a critic to stress-test an approach.
- When an auditor needs to judge whether executor claims are supported.
- When a design "sounds reasonable" but hasn't been checked against failure modes.

Do NOT use for raw fact gathering — use [[mastermind-codegraph-research]] first when symbol existence, callers, imports, file paths, or blast radius are unknown. Do NOT use to implement fixes; this produces critique, not code.

## Inputs

- **Proposal** — the design, spec, report, or plan under review.
- **Evidence** — codegraph facts, files, test results, logs, or an explicit "evidence unavailable".
- **Scope** — what the review may challenge.
- **Lens** (optional) — security, performance, simplicity, migration safety, API compatibility, or testing.

If evidence is missing, say so. Don't invent facts.

## Review protocol

Run in order.

1. **Evidence** — Which claims are backed by local evidence vs assumption? Which named files/symbols/callers/contracts are unverified? Is the design relying on memory or guesswork? Code structure named without evidence is a finding.
2. **Contract** — Does this change an API, data shape, permission boundary, event/CLI/config contract, or persisted behavior? Are existing consumers covered? Is backward compatibility explicit? Are error shapes and edge cases preserved? Hidden contract changes are high severity.
3. **Failure mode** — What happens on retry, partial failure, concurrency, stale data, empty/malformed/duplicated/out-of-order input, or a slow/missing/misbehaving dependency? Report only the plausible ones.
4. **Scope** — Solving the stated problem or adding future-proofing? Is every abstraction justified by ≥ 2 current use cases? Is unrelated cleanup mixed in? Is the path larger than the problem requires? Scope creep is a finding even when the extra work seems useful.
5. **Test & verification** — Would the proposed tests fail *before* the fix? Do they cover the contract that can break? Are integration paths covered where unit tests aren't enough? Are VERIFY commands concrete and runnable? "Run tests" is not a sufficient plan for behavior-changing work.
6. **Rollback & observability** — Safely revertible? Does it need a migration/backfill/feature flag? Would production failure be visible? Require observability only when the risk justifies it.

## Severity

- **P0** — security breach, data loss, money-movement error, or irreversible production breakage.
- **P1** — likely correctness break, broken public/internal contract, unsafe migration, or unbounded blast radius.
- **P2** — missing evidence, weak tests, rollback ambiguity, or likely maintenance issue.
- **P3** — clarity, naming, small simplification, or docs.

Don't inflate severity. If uncertain, say what evidence would change it.

## Verdict

Exactly one:

- **ship it** — no blocking issues.
- **ship with caveats** — non-blocking issues only; proceed after noting them.
- **revise** — one or more P1/P2 must be addressed before execution/approval.
- **rethink** — the approach is likely wrong or unsafe; redesign required.
- **insufficient evidence** — required facts are missing; the critique can't complete.

## Output

```markdown
## Critical review

**Verdict:** ship it | ship with caveats | revise | rethink | insufficient evidence
**Lens:** <default | security | performance | simplicity | migration | API | testing>
**Scope reviewed:** <one sentence>

### Findings

| Severity | Finding | Evidence | Required change |
|---|---|---|---|
| P1 | <specific problem> | <fact, citation, or "missing evidence"> | <what must change> |

### Assumptions challenged
- `<assumption>` → <why it's risky or what evidence is missing>

### What looks sound
- <only concrete strengths backed by evidence>

### Not reviewed
- <anything outside scope or blocked by missing evidence>
```

Rules:

- Max 7 findings unless the design is broadly unsafe.
- No generic advice. No praise unless evidence-backed.
- Don't propose a larger architecture unless the current one fails.
- Prefer "missing evidence" over speculation.
- No issues → `ship it` with a short explanation.

## Related skills

- [[mastermind-codegraph-research]] — gather the structural facts this review verifies against
- [[mastermind-structured-report-contract]] — the executor/auditor report tails a review may scrutinize
