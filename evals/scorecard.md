# Eval scorecard

Only complete model-backed suite runs appear here. The four legacy suites were
last run together on 2026-07-31. Critic and researcher were run again on
2026-08-25 with Claude Code 2.1.231.

| Suite | Model | Date | Result | First pass | Elapsed |
|---|---|---|---:|---:|---:|
| researcher | haiku | 2026-08-25 | 3/3 | 3/3 | 36.3 s |
| critic | opus | 2026-08-25 | 5/5 | 5/5 | 172.0 s |
| critic (pre-lean baseline) | opus | 2026-08-25 | 5/5 | 5/5 | 297.4 s |
| auditor | opus | 2026-07-31 | 9/9 | 8/9 | 2,097.0 s |
| critic | opus | 2026-07-31 | 5/5 | 5/5 | 283.0 s |
| intake | sonnet | 2026-07-31 | 5/5 | 5/5 | 98.4 s |
| workflow | sonnet | 2026-07-31 | 51/56 | 51/56 | 1,005.4 s |
| workflow | sonnet | 2026-07-30 | 45/47 | 45/47 | 778.9 s |
| workflow | sonnet | 2026-07-19 | 36/36 | 36/36 | 525.6 s |

## Current evidence

**Critic token gate, 2026-08-25.** The current and pre-lean runs used the same
cases, resolved model, and Claude CLI version. All five cases passed in both.

| Metric | Pre-lean | Current |
|---|---:|---:|
| Context tokens p50 | 7,497 | 3,891 |
| Context tokens p95 | 7,643 | 4,037 |
| Output tokens, total | 21,186 | 12,231 |
| Elapsed | 297.4 s | 172.0 s |

The executable gate requires no quality regression and lower context-token
p50/p95. Output and elapsed time are observations. Five cases are too few for a
variance or durable performance claim. The pre-lean baseline was captured from
console output and records its API-duration limitation.

**Researcher, 2026-08-25.** Three disposable Git cases ran with live mmcg.
The required graph-first and source-read tool checks held. Across the suite:
10 turns, 2,772 output tokens, context-token p50/p95 of 16,569/35,274, and
Claude CLI reported cost of $0.0325. There is no pre-lean researcher comparison.

## Historical runs

- **2026-07-31:** Auditor used real Git fixtures and live mmcg; `a-007` needed
  one retry. The five workflow failures in the 51/56 run were assertion
  problems: forbidden propositions quoted to reject them, wording variants,
  or an incidental token. All nine newly added workflow cases passed on first
  attempt.
- **2026-08-11:** A full rerun stopped before inference because the local
  OAuth session had expired. This was an authentication failure, not a model
  failure. The earlier `c-002` critic row predates the runner's prompt-isolation
  repair and is historical evidence only.
- **2026-07-30:** The 45/47 workflow run exposed two forbidden-phrase checks
  that matched correct denials or finding labels. The earlier 34/39 run missed
  required phrases despite correct behavior in several cases. Targeted repairs
  diagnosed these checks; the historical complete-run scores remain as shown.

## How to read the scores

- A case tests one short scenario. Green workflow cases show the instruction
  can be followed there; they do not prove it survives a long implementation.
- A required phrase can miss a correct paraphrase. A forbidden fragment can
  match a denial. Inspect the answer before treating either as a behavior
  regression.
- A targeted rerun diagnoses a case but does not change a suite result.
- Auditor results need a valid mmcg index and structured YAML verdict.
  Missing index setup is a failure.
- Runs before `4c338b6` (2026-06-10) used prose verdict matching and are not
  comparable with current runs.

A live-diff comment audit on 2026-07-30 returned `clean` for a 308-line Rust
diff: 7 added comments, 0 flagged. It was one tool-free observation, not a
suite score. The auditor ablation has not been run for this snapshot; no causal
quality-uplift claim is recorded.
