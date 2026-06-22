# Eval results

Last run: 2026-06-22 · `da12de0` · opus. Cases live in `*.jsonl`; run with
`bash evals/run-verified.sh` or `python evals/runner.py [--suite X] [--case Y]`.

| suite | result | first_pass | note |
|---|---|---|---|
| auditor (9) | 9/9 | 8/9 | one case flakes the sentinel block each run; runner retries once |
| critic (5) | 5/5 | 5/5 | — |
| intake (5) | 5/5 | 5/5 | — |

**Trust the numbers only if** the mmcg index is present (hard-fail otherwise),
the verdict is read from the structured YAML tail (not prose), and fixtures
carry no answer-leaking comments. Results before `4c338b6` (2026-06-10) used
prose matching — don't cite them.

**Sentinel flake:** each auditor run, one case ends before emitting
`<!-- mastermind:audit-begin -->` (reasoning is correct). Which case varies
(a-004, then a-002); `first_pass` tracks it honestly.

**Ablation** (`ablation.py`): vanilla (Claude + grep, no mmcg/auditor prompt)
vs the real auditor over planted-defect cases — the uplift is the codegraph's
value. Not run yet: `python evals/ablation.py --with-mastermind`.
