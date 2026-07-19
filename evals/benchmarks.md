# Eval results

Latest runs: 2026-07-19 · working tree. Critic and auditor used opus; intake and
workflow used sonnet. Cases live in
`*.jsonl`; run with `bash evals/run-verified.sh` or
`python evals/runner.py [--suite X] [--case FULL_CASE_ID]`.

| suite | result | first_pass | note |
|---|---|---|---|
| auditor (9) | 9/9 | 9/9 | real git fixtures + live mmcg; no sentinel retry used |
| critic (5) | 5/5 | 5/5 | — |
| intake (5) | 5/5 | 5/5 | full suite after separating passthrough metadata from refined output |
| workflow (18) | 18/18 | 17/18 | final full run was 17/18; one redundant phrase assertion was removed and its targeted rerun passed |

**Trust the numbers only if** the mmcg index is present (hard-fail otherwise),
the verdict is read from the structured YAML tail (not prose), and fixtures
carry no answer-leaking comments. Results before `4c338b6` (2026-06-10) used
prose matching — don't cite them.

The runner now supports bounded failure output and alternative phrase groups,
so a correct `leave/keep/preserve` answer does not fail solely on wording.

**Ablation** (`ablation.py`): vanilla (Claude + grep, no mmcg/auditor prompt)
vs the real auditor over planted-defect cases — the uplift is the codegraph's
value. Not run yet: `python evals/ablation.py --with-mastermind`.
