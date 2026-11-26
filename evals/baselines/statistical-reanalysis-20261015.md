# Statistical reanalysis of retained development campaigns

Purpose: measure what the six retained development campaigns of 2026-10-13/14
can and cannot establish, using exact small-sample methods. No model was invoked;
no outcome was regraded; no attempt was added or removed. This report adds no new
experimental evidence and qualifies no product benefit.

## Inputs

| Input | SHA-256 |
|---|---|
| [repo-map-effort-20261013.json](repo-map-effort-20261013.json) | `69473e0b…679372` |
| [repo-map-graph-20261013.json](repo-map-graph-20261013.json) | `57f0efb4…dd1184` |
| [repo-map-boundaries-v2-20261013.json](repo-map-boundaries-v2-20261013.json) | `c1e9c936…14a1c3` |
| [repo-map-factorial-20261013.json](repo-map-factorial-20261013.json) | `9b011607…3f9c56da` |
| [source-quotes-20261014.json](source-quotes-20261014.json) | `ac8e12e2…3fb3033` |
| Local `repo-map-effort-20261013/effective-results.json` | `e130361b…1fc1` |
| Local `repo-map-graph-20261013/effective-results.json` | `29c618bd…8be18` |
| Local `repo-map-graph-20261013/recovered-effective-results.json` | `7243a16f…9196` |
| Local `repo-map-boundaries-v2-20261013/effective-results.json` | `30e045f3…325c` |
| Local `repo-map-factorial-20261013/effective-results.json` | `ab11e9fd…0793a71` |
| Analysis script `reanalyze.py` | `1245dab2…e931e99ff` |
| Analysis output `output.txt` | `9c004b55…d9c595a0` |

Script and full output: local `.mastermind/research/statistical-reanalysis-20261015/`.
Repository revision: `b0493cc`.

## Method

| Quantity | Method | Unit |
|---|---|---|
| Outcome change | Exact McNemar (binomial on discordant resolved pairs); unknowns bounded both ways | Paired case × repetition slot |
| Resource change | Geometric-mean ratio of task-level mean log ratios; exact two-sided sign-flip randomization over 8 tasks; t-interval (7 df) as a descriptive range | Task (repetitions averaged) |
| Variance structure | One-way random effects on log totals and on log ratios | Task with 2 repetitions |
| Campaign spread | χ² homogeneity of the same nominal condition across campaigns | Campaign of 16 slots |
| Sample size | Normal approximation for paired binary non-inferiority and for log-ratio effects | Independent task |

Resources use the last completed attempt per slot; failed transport attempts stay
in the original all-attempt totals and are excluded here. The smallest attainable
sign-flip p-value with 8 tasks is 2/256 = 0.0078. Inference is conditional on
these 8 tasks, all about Mastermind's own source; it does not extend to a task
population. Outcomes are the retained Astra model assessments, not human labels.

## Resource effects

Geometric-mean ratio candidate/baseline; `p` is the exact sign-flip p-value.

| Contrast | Total tokens | Uncached input | Output | Producer run time | MCP calls |
|---|---|---|---|---|---|
| Low vs high effort | **−34.2%** [−45.0, −21.2], p=0.008 | −18.3% [−34.5, +2.0], p=0.086 | **−52.8%** [−57.6, −47.4], p=0.008 | **−44.5%** [−52.0, −35.8], p=0.008 | **−21.8%** [−34.2, −7.0], p=0.023 |
| Graph vs lexical ranking | +9.1% [−8.6, +30.2], p=0.30 | +1.2%, p=0.85 | −3.3%, p=0.38 | −4.9%, p=0.60 | +4.6%, p=0.29 |
| Boundary instruction v2 | +5.0%, p=0.41 | −5.2%, p=0.41 | **+20.6%** [+13.8, +27.9], p=0.008 | **+23.9%** [+4.7, +46.6], p=0.047 | +2.2%, p=0.29 |
| Bundle vs no map | −6.0% [−15.6, +4.6], p=0.20 | −9.9% [−18.7, −0.1], p=0.062 | +5.8%, p=0.23 | +5.0%, p=0.49 | −5.3%, p=0.28 |
| Map-only vs no map | −10.3% [−18.6, −1.1], p=0.047 | −5.6%, p=0.20 | +6.8%, p=0.086 | −1.5%, p=0.81 | +0.3%, p=0.90 |
| Instructions-only vs no map | −1.7%, p=0.69 | +2.0%, p=0.74 | −0.5%, p=0.85 | −7.8%, p=0.24 | +3.8%, p=0.45 |
| Mandatory source quotes | −6.1%, p=0.32 | +1.3%, p=0.80 | **+53.9%** [+37.3, +72.4], p=0.008 | **+43.4%** [+29.0, +59.3], p=0.008 | +0.8%, p=0.87 |

The low-effort latency effect (−44.5% producer run time) was not reported in the
original effort report. No multiplicity correction is applied across these 35 cells;
treat p-values near 0.05 as descriptive.

## Outcome effects

| Contrast | Baseline S/U/? | Candidate S/U/? | S→U | U→S | Exact McNemar p | Acceptance change bounds over unknowns |
|---|---|---|---:|---:|---:|---|
| Low vs high effort | 11/0/5 | 13/2/1 | 1 | 0 | 1.00 | −18.8 to +18.8 pp |
| Graph vs lexical | 13/0/3 | 13/2/1 | 1 | 0 | 1.00 | −18.8 to +6.2 pp |
| Boundary v2 | 14/2/0 | 10/2/4 | 2 | 2 | 1.00 | −25.0 to 0.0 pp |
| Bundle vs no map | 14/1/1 | 14/2/0 | 2 | 1 | 1.00 | −6.2 to 0.0 pp |
| Map-only vs no map | 14/1/1 | 12/1/3 | 1 | 1 | 1.00 | −18.8 to +6.2 pp |
| Instructions-only vs no map | 14/1/1 | 13/2/1 | 2 | 1 | 1.00 | −12.5 to 0.0 pp |
| Mandatory quotes | 15/1/0 | 14/2/0 | 1 | 0 | 1.00 | −6.2 pp |

No contrast has a detectable outcome difference. The earlier "13/16 vs 11/16"
for low effort reflects five unresolved high-effort reviews, not a measured gain.

## Why the outcomes cannot resolve

| Finding | Measurement | Consequence |
|---|---|---|
| The same condition varies only as binomial noise | Lexical bundle, Sol/high: 14, 13, 14, 11, 15 of 16; observed sd 1.52 vs binomial 1.48; χ²=4.23, df=4, p=0.38 | The ±3/16 spread needs no campaign effect; n=16 cannot resolve differences below about 20 pp |
| Judge disagreement equals arm discordance | Sol vs Astra on identical answers: 2/14 = 14.3% (Wilson 4.0–39.9%); arms on resolved pairs: 15/92 = 16.3% (10.1–25.2%) | Outcome flips between arms are of the size expected from judging alone |
| Four tasks carry the outcome signal | Non-satisfied arm-slots: concept-match 10/28, documentation-concepts 10/28, reference-removal 7/28, document-evidence 6/28; the other four tasks ≤ 2/28 | Half the corpus is at ceiling and adds almost no outcome information |
| Unresolved reviews | 21 of 224 analysed arm-slots | Unknowns dominate several bounds above |

## Variance structure and sample size

| Quantity | Estimate | Use |
|---|---|---|
| Log total tokens, ICC by task | 0.79–0.94 per arm | Tasks differ far more than runs in absolute size; always pair by task |
| Log token *ratio*, run-to-run sd | 0.18–0.29 (median 0.25) | Run noise dominates resource effects |
| Log token *ratio*, task-to-task sd | 0–0.08 (median 0.03) | Resource effects were nearly homogeneous across these tasks; poorly estimated with 8 tasks |
| Pooled discordance on resolved pairs | 0.163 (Wilson upper 0.252) | Drives the outcome sample size |

| Claim to establish (80% power) | Required independent tasks |
|---|---|
| −10% total tokens, 1 repetition per task (two-sided 0.05; s_w 0.25, s_b 0.05) | 47 |
| −10% total tokens, 2 repetitions | 25 |
| −10% total tokens, 4 repetitions | 13 |
| −10% total tokens, 8 repetitions | 8 |
| −20% total tokens, 2 repetitions | 4–8 |
| Acceptance non-inferiority, 10 pp margin (one-sided 0.05), true difference 0 | 101–156 |
| Acceptance non-inferiority, 5 pp margin | 404–623 |

Resource claims are reachable with the existing development tasks and more
repetitions. Acceptance non-inferiority at 10 pp needs roughly an order of
magnitude more independent tasks than any retained campaign, unless outcome
measurement itself becomes more informative (lower judge noise, fewer unknowns,
tasks away from the ceiling, criterion-level outcomes).

## Limits

| Limit | Treatment |
|---|---|
| 8 development tasks, one repository | All inference is conditional on these tasks |
| Model judge | Astra outcomes are proxies; human reference remains incomplete |
| Task-to-task variance | Estimated from 8 tasks × 2 repeats; truncated at zero in several contrasts |
| Completed-slot resources | One graph transport failure has unknown usage; all-attempt totals stay in the original report |
| Normal approximations | Sample sizes are planning estimates; simulate the registered design before qualification |
| Multiplicity | 35 resource cells and 7 outcome contrasts are uncorrected |

## Reproduce

```sh
python3 -B .mastermind/research/statistical-reanalysis-20261015/reanalyze.py
```

Requires the local retained campaign directories under `.mastermind/research/`.
The script uses only the standard library and runs internal self-checks first.
