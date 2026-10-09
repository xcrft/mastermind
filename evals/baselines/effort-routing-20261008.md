# Codex effort-routing calibration

Compare fixed `max` with the opt-in `bounded_readonly_v1` request heuristic.
The router uses public task features only; it makes no model call and receives
no private review key. It selects `high` for two bounded read-only tasks and
retains `max` for state and removal tasks.

**Decision:** keep the policy experimental. The complete-answer review leaves
one count/truncation criterion unresolved in each arm. No material false claim
was found, but preserved quality across the full corpus is unproved. Point
tokens per accepted task and quality-gated savings remain undefined.

## Inputs and outcome

| Contract | Retained value |
|---|---|
| Attempts | Four public source tasks, two repeats, 16 completed attempts in balanced order |
| Requested runtime | Codex 0.162.0-alpha.2, `gpt-6.1-sol`, current subscription |
| Control | Same local 0.3.4 instruction and frozen native source/tool binary in both arms |
| Limits | No time, output-token or turn budget; byte/integrity/tool guards remain |
| Review | Original request criteria; maintenance agent, neither blinded nor independent |
| Accepted outcomes | 7–8/8 control; 7–8/8 adaptive |
| Production | Default effort and instructions unchanged; no promotion |

## Observed resources

Each row is one task with two observations per arm. Tokens per attempt count
reported input, cache reads/writes and output; they are not billing cost or
tokens per accepted task. Time includes preparation and the complete attempt,
but router bookkeeping is unmeasured.

| Task | Requested effort | Total p50, seconds | Change | Tokens per attempt | Change | Accepted outcomes |
|---|---|---:|---:|---:|---:|---|
| task-phase-continuity-01 | max → max | 496.65 → 269.42 | -45.75% | 399,920 → 378,835 | -5.27% | 2/2 → 2/2 |
| document-evidence-boundaries-01 | max → high | 264.35 → 84.46 | -68.05% | 197,821 → 127,236 | -35.68% | 2/2 → 2/2 |
| callees-definition-boundaries-01 | max → high | 283.97 → 102.23 | -64.00% | 346,732 → 310,550 | -10.43% | 1–2/2 → 1–2/2 |
| reference-removal-evidence-01 | max → max | 324.79 → 344.96 | +6.21% | 321,215 → 330,417 | +2.86% | 2/2 → 2/2 |

Across all eight observations per arm, total p50 is
301.66 → 183.56 seconds
(-39.15%); observed p95 is
540.37 → 360.53 seconds
(-33.28%).
Same-`max` controls also vary substantially. Global differences cannot isolate
the effort effect, provider load, cache or run order. These quantiles are
descriptive sample values, not population tail estimates.

## Inspect and qualify

Read the [sealed comparison](effort-routing-20261008.json) for every attempt,
source/runtime digest, criterion, partial source-key coverage, unknown omission
and resource observation. [Harness validation](effort-routing-harness-20261008.json)
retains 389 passing deterministic tests and four detected guard mutations.
Transport checks do not establish semantic quality.

Use the [campaign instructions](../benchmark/README.md) to prepare a new run;
preserve this report. Qualify on unseen repositories and independently reviewed
audits/fixes before changing a default. A future completion check or escalation
policy needs its own frozen comparison, including verification and retry costs.

Profile delivery, native prompt refinement, applied fixes, actual served
model/effort and subscription billing are unmeasured here. Every planned model
attempt contributes to the optimization cost; development/review inference
and amortization remain unknown.
