---
name: mastermind-investigation-ledger
description: Investigate unknown bugs with an explicit hypothesis ledger and evidence-driven probes before drafting a task spec. Triggers on unexplained failures, ambiguous test failures, regressions, or bug reports without a supported root cause.
metadata:
  version: 0.1.1
  authors:
    - mastermind
  tags:
    - debugging
    - investigation
---

# Investigation ledger — confirm root cause before specifying

When a bug, test failure, or regression has **no known cause**, diagnose before
specifying the fix. The ledger prevents premature closure while allowing
independent evidence gathering when it is safe and useful.

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
Next decision point: <the smallest probe or independent probe set that best splits the live hypotheses>
```

- A hypothesis is **`confirmed`** only when `evidence_for` is populated AND `evidence_against` has been checked. No `evidence_against` = premature closure; keep it `open`.
- Ruling out every alternative does NOT confirm the survivor — it may mean all hypotheses are wrong.

## The loop

1. Read the ledger. Don't pre-decide which hypothesis is right.
2. Run the smallest probe that changes the decision. Independent read-only
   probes may run together when neither depends on the other's result; record
   each result separately.
3. Fold results back in: update `evidence_for` / `evidence_against` and set the
   next decision point.
4. Repeat until the causal explanation is supported. A failure may have more
   than one contributing confirmed cause; describe their relationship rather
   than forcing a single winner.

## When to stop and open a spec

Stop when the root cause or contributing causes are supported and the best
explanation names concrete locations. Then:
1. Put the user-visible corrected behavior in **Goals** and the supported cause
   in a concise **Problem / Root Cause** note.
2. Link or summarize only ruled-out evidence that prevents repeated work; do
   not paste the full investigation transcript into the implementation spec.

After ~5 probes without a confirmed hypothesis (or sooner when no distinguishing test remains): stop and escalate to the user with the full ledger and ruled-out table. Bounded investigation beats an infinite probe loop. Don't guess a cause; don't open a spec on an `open` or `weakened` hypothesis.

## Anti-patterns

- **Skipping investigation because you "know" the answer** — if you can't populate `evidence_against`, you're guessing.
- **Running dependent probes in parallel** — parallelize only independent reads or tests whose interpretation does not depend on another result.
- **Treating "no other hypothesis survived" as evidence_for** — it isn't.

## Related skills

- [[mastermind-codegraph-research]] — ground structural claims in mmcg, not memory
- [[mastermind-structured-report-contract]] — the executor/auditor report tail you produce or consume
