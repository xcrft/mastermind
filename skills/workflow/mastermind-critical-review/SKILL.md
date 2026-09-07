---
name: mastermind-critical-review
description: Stress-test a proposed design, task spec, implementation plan, or executor report for false assumptions, broken contracts, scope creep, missing evidence, and high-risk failure modes. Use before drafting sensitive specs, before approving a plan, or when a critic/auditor needs a compact review rubric.
metadata:
  version: 0.3.0
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

This is the rubric; the spawnable agent that applies it is the
`mastermind-critic` subagent in independent context.

## When to use

- Before approving a non-trivial task spec.
- Before accepting a design that touches auth, billing, migrations, public APIs, data loss, or rollback complexity.
- When a planner asks a critic to stress-test an approach.
- When an auditor needs to judge whether executor claims are supported.
- When a design "sounds reasonable" but hasn't been checked against failure modes.

Do NOT use for raw fact gathering — use [[mastermind-codegraph-research]] first when symbol existence, callers, imports, file paths, or blast radius are unknown. Do NOT use to implement fixes; this produces critique, not code.

**Security scope:** if the review surfaces security-sensitive scope (auth, tools, secrets, delegation, supply chain, prompt injection), don't go deep inline — spawn `mastermind-security-auditor` and fold its verdict into the critique.

## Inputs

- **Proposal** — the design, spec, report, or plan under review.
- **Evidence** — codegraph facts, files, test results, logs, or an explicit "evidence unavailable".
- **Scope** — what the review may challenge.
- **Lens** (optional) — security, performance, simplicity, migration safety, API compatibility, or testing.

If evidence is missing, say so. Missing mmcg alone does not invalidate a review
when source, test, or runtime evidence establishes the required facts. An
unsupported claim is unknown; a claim contradicted by evidence is a finding.

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

Use the same seven dimensions as `mastermind-critic`: correctness, performance
and scale, observability, compatibility, YAGNI, AI slop, tests and docs. Score
each `pass`, `concern` (a concrete guard/detail is needed), `fail` (an evidenced
defect), or `unknown` (a required fact is missing or contradictory). Mark an
irrelevant dimension pass with a reason. Never invent concerns or alternatives
to fill rows. Consider plausible alternatives only when a real choice exists.

Aggregate in this order, independent of the severity labels on findings:

1. Two or more evidenced fails, or a correctness fail invalidating the approach
   → **rethink**.
2. Otherwise one evidenced fail → **revise**.
3. Otherwise any unknown → **insufficient evidence**.
4. Otherwise any concern → **ship with caveats**.
5. Otherwise all pass → **ship it**.

Unknowns do not erase known failures or prove a design wrong. A P2 evidence gap
alone cannot force `revise`. Give the smallest factual probe needed to resolve
each unknown. `insufficient evidence` blocks acceptance until those facts are
checked; it is not approval with caveats.

## Output

```markdown
## Critical review

**Lens:** <default | security | performance | simplicity | migration | API | testing>
**Scope reviewed:** <one sentence>

### Dimensions

| Dimension | Verdict | Evidence |
|---|---|---|
<all seven rows: pass / concern / fail / unknown, with evidence or missing fact>

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

### Epistemic envelope
- **Observed:** <direct evidence used by the verdict>
- **Inferred:** <bounded conclusion and why it follows>
- **Confidence:** high | medium | low — <reason>
- **Would change the verdict:** <specific evidence or failed/passing proof>

## Verdict
<ship it | ship with caveats | revise | rethink | insufficient evidence> — <reason>
```

Rules:

- Max 7 findings unless the design is broadly unsafe.
- No generic advice. No praise unless evidence-backed.
- Don't propose a larger architecture unless the current one fails.
- Prefer "missing evidence" over speculation.
- No issues and no material unknowns → `ship it` with a short explanation.
- End with exactly one `## Verdict` section; do not repeat it in a summary.

## Related skills

- [[mastermind-codegraph-research]] — gather the structural facts this review verifies against
- [[mastermind-structured-report-contract]] — the executor/auditor report tails a review may scrutinize
