# Retrieval instruction calibration

Neither candidate is promoted to production defaults. These are descriptive results
from four public source-reading tasks, with two counterbalanced repetitions.

| Experiment | Manual acceptance | Run seconds, baseline → candidate | Reported tokens, baseline → candidate | Decision |
|---|---|---|---|---|
| routing: baseline → routed | 8/8 → 8/8 | 919.2 → 904.1 (-1.6%) | 2,838,551 → 3,011,075 (+6.1%) | Resource targets not met |
| batching: single → batch | 8/8 → 7/8 | 973.3 → 888.7 (-8.7%) | 2,926,734 → 2,680,271 (-8.4%) | Quality loss |

Acceptance is an unblinded implementation-agent assessment of the original request.
Source-key omissions and unknowns remain in the records. Passing these reviews does
not qualify a product benefit or prove that batching caused a difference.

Batch resources per manually accepted answer increased by 4.4% in run time and
4.7% in reported tokens because one outcome was rejected. Aggregate savings do
not establish an accepted-task efficiency gain.

| Experiment / condition | Uncached input tokens | Cached input tokens | Output tokens |
|---|---|---|---|
| routing / baseline | 496,130 | 2,314,240 | 28,181 |
| routing / routed | 467,830 | 2,515,840 | 27,405 |
| batching / single | 420,248 | 2,476,544 | 29,942 |
| batching / batch | 432,348 | 2,220,544 | 27,379 |

The adapter subtracts cached input from uncached input. Total-token and uncached-input
changes can differ; subscription billing cost is not reported.

## Actual retrieval use

| Experiment / condition | MCP calls | Exact lookup calls | Single / batch | Names requested | Source reads | Returned / repeated lines |
|---|---|---|---|---|---|---|
| routing / baseline | 214 | 19 | 2 / 17 | 95 | 158 | 17718 / 1311 |
| routing / routed | 221 | 17 | 0 / 17 | 65 | 169 | 17942 / 2143 |
| batching / single | 242 | 56 | 56 / 0 | 56 | 157 | 16146 / 1590 |
| batching / batch | 204 | 18 | 3 / 15 | 71 | 158 | 16556 / 1786 |

The common instruction requests `top=10` per name. Actual requested limits and
batch sizes are retained per attempt; fewer requests do not prove fewer model rounds.
Twenty-four native batch comparisons produced 96 identical per-name replies, including
counts, missing names, selectors and truncation. This checks reply equivalence only.

## Quality boundary found

One batch answer predicts: “A subsequent post-only retry encounters the pre-flight-required gate.”
That prediction requires the `held/run_preflight` write to succeed. If the earlier
`audit_required/run_audit` write succeeds and the later write fails, the next call can
return `PostBroken` again. The answer leaves write success unknown but predicts the
gate unconditionally; the outcomes criterion is rejected.

Counterevidence: `mcp/servers/mmcg/src/run_task.rs:2013-2022`, `:2982-2990`, `:3032-3044`.
For the next candidate, distinguish attempted writes, persisted state and the
conditions required for a later transition. Freeze that change before another comparison.

## Reproduction and limits

- Source baseline: `9588cc0390e745948cbcbc79f4c9f67d3b6f95b8`. Forty key anchors were reviewed; their bytes
  are unchanged, with six moved line anchors. This is maintenance review.
- Frozen experiment inputs: `6da6a731a6cf9b4f2343e1f84b5a3229ffad125a`. Adapter and instruction
  hashes, source bytes and all 32 completed attempts are bound in the JSON report.
- Requested runtime: Codex subscription, `gpt-6.1-sol/high`. Served model and effort are unknown.
- Model time, turn and output-token limits are disabled; transport byte caps remain active.
- Reported token totals include repeated cached context. Billing cost remains unknown.
- Wall time includes startup, model, transport and host effects; it does not isolate a stage.
- Profile mining, profile application and applied code-fix correctness are outside this experiment.
- Checks: 410 Python tests pass; validator reports 40 artifacts, zero errors and warnings.

[Bound results and per-attempt records](context-routing-20261010-public.json)
