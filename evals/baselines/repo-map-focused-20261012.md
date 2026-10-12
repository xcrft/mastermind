# Focused repo-map pilot

**Decision: retain as an experiment; do not promote.** On the two development tasks, total tokens rose 36.4% and calls rose 15.3% versus the fresh baseline. Model reviews accepted both baseline answers and one of two map answers. The rejected map answer incorrectly treats managed write/refresh admission as a requirement to open any index, contradicting `store.rs:2780–2783` and `store.rs:2804–2806`, which return read-only stores.

Fresh paired baseline/map comparison on four tasks: two development and two holdout. One attempt per task/condition, order alternated; model source reviews are provisional, human reference pending.

Intervention: grouped shortlist, 1,600-byte budget, at most three files/three definitions each; direct source reads and uncovered-range instruction. This is a bundle comparison.

| Scope / metric | Baseline | Focused map | Change |
|---|---:|---:|---:|
| all / total_reported_tokens | unknown | 2059250 | unknown |
| all / mcp_calls | 163 | 183 | +12.3% |
| all / returned_lines | 8173 | 12134 | +48.5% |
| all / repeated_lines | 218 | 480 | +120.2% |
| all / blind satisfied | 2/4 | 3/4 | provisional |
| development / total_reported_tokens | 741917 | 1012164 | +36.4% |
| development / mcp_calls | 72 | 83 | +15.3% |
| development / returned_lines | 3938 | 4879 | +23.9% |
| development / repeated_lines | 39 | 201 | +415.4% |
| development / blind satisfied | 2/2 | 1/2 | provisional |
| holdout / total_reported_tokens | unknown | 1047086 | unknown |
| holdout / mcp_calls | 91 | 100 | +9.9% |
| holdout / returned_lines | 4235 | 7255 | +71.3% |
| holdout / repeated_lines | 179 | 279 | +55.9% |
| holdout / blind satisfied | 0/2 | 2/2 | provisional |

Matched complete pairs: 3/4. This conditional subset excludes incomplete pairs and does not establish all-attempt spend.

| Matched complete metric | Baseline | Focused map | Change |
|---|---:|---:|---:|
| total_reported_tokens | 1831797 | 1502092 | -18.0% |
| mcp_calls | 145 | 125 | -13.8% |

Producer attempts: 8; failures: 1. Model review attempts: 7; valid source-anchored assessments: 6/7 attempted reviews (8 planned producer slots). No human reference labels have been entered; 7 answers are available in the human packet.

The baseline has two satisfied model assessments, one unresolved source anchor and one producer timeout without an answer. The map has three satisfied and one unsatisfied model assessments. Unresolved/failed slots remain in denominators and are not semantic rejections.

The table uses effective task slots. If a failed attempt has unreported usage, all-attempt token comparisons remain unknown in the JSON; completed-answer figures do not establish full spend.

mcp_calls counts unique calls seen, including started calls; completed calls and pending timeline state are retained separately in the JSON. Delivered lines are delivery accounting, not semantic coverage.

The original pilot remains immutable. Its historical +13.6% tokens and +22.5% calls came from two tasks repeated twice; compare the new candidate with its fresh baseline.

## Reproduce and recover

Prerequisites: the retained frozen source/index/native/CLI bindings, existing Codex subscription and the installed Python runtime. Inspect input-seal.json before running models. Do not edit existing attempts.

```sh
python3 -B .mastermind/research/repo-map-focused-20261012/pilot.py --run
python3 -B .mastermind/research/repo-map-focused-20261012/review.py
python3 -B .mastermind/research/repo-map-focused-20261012/summarize.py
```

The controller skips completed result records and retains at most one qualifying capacity retry. After interruption, do not remove partial traces or rerun productive attempts; audit the interruption and create a new input revision if transport recovery is needed. Human packet: `.mastermind/research/repo-map-focused-20261012/human-review/`.

Limits: two held-out tasks, no repeats, same model family as producer/reviewer, requested model identity only, unobserved billing and model rounds. This does not qualify a production default.
