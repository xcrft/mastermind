---
name: mastermind-investigation-ledger
description: Investigate unknown bugs with a hypothesis ledger and one-probe-at-a-time loop before drafting a task spec. Triggers on unexplained failures, ambiguous test failures, regressions, or bug reports without confirmed root cause.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - debugging
    - investigation
---

# Investigation ledger — confirm root cause before specifying

When a bug, test failure, or regression has **no known cause**, don't open a spec. Diagnose first with a hypothesis ledger and a one-probe-at-a-time loop. Opening a spec on a misdiagnosed bug burns a full executor + auditor cycle.

## When to use

Use when:
- "X is broken" with no stated why.
- A stack trace points to ≥ 2 plausible causes.
- Behavior changed and no obvious commit explains it.
- You catch yourself guessing ("probably the cache", "likely a race") without evidence.

Do NOT use when:
- The cause is already known (a typo, a wrong constant, a confirmed missing import) → go straight to a spec.
- It's a feature request, not a failure.
- The fix is a self-evident one-liner.

## The ledger

Track every hypothesis explicitly:

```markdown
| # | hypothesis | status | evidence_for | evidence_against |
|---|---|---|---|---|
| 1 | <concrete, falsifiable cause> | open / weakened / confirmed | <facts> | <facts> |

Current best explanation: <a concrete code location, config value, or dependency>
Next probe: <exactly ONE — the single test/query that best splits the live hypotheses>
```

- A hypothesis is **`confirmed`** only when `evidence_for` is populated AND `evidence_against` has been checked. No `evidence_against` = premature closure; keep it `open`.
- Ruling out every alternative does NOT confirm the survivor — it may mean all hypotheses are wrong.

## The loop

1. Read the ledger. Don't pre-decide which hypothesis is right.
2. Run **exactly one** probe — the Next probe. Uncoordinated parallel probes create conflicting evidence the ledger can't cleanly absorb.
3. Fold the result back in: update `evidence_for` / `evidence_against`, set the next probe.
4. Repeat until exactly one hypothesis is `confirmed`.

## When to stop and open a spec

Stop when one hypothesis is `confirmed` and the best explanation names a concrete location. Then:
1. Copy the root cause into the spec's **Goal**.
2. Copy the ruled-out table into the spec's **Notes** — the executor must not re-investigate.

After ~5 probes without a confirmed hypothesis (or sooner when no distinguishing test remains): stop and escalate to the user with the full ledger and ruled-out table. Bounded investigation beats an infinite probe loop. Don't guess a cause; don't open a spec on an `open` or `weakened` hypothesis.

## Anti-patterns

- **Skipping investigation because you "know" the answer** — if you can't populate `evidence_against`, you're guessing.
- **Running your own probes in parallel** — the probe sequence is deliberate; each result informs the next.
- **Treating "no other hypothesis survived" as evidence_for** — it isn't.

## Related skills

- [[mastermind-codegraph-research]] — ground structural claims in mmcg, not memory
- [[mastermind-structured-report-contract]] — the executor/auditor report tail you produce or consume
