# Eval scorecard

This file records behavioral regression runs. It is not a model benchmark or a
coverage percentage: case sets differ by suite, and elapsed time depends on the
model, machine, and fixture setup.

Latest complete runs: 2026-07-19, uncommitted working tree.

| suite | model | result | first pass | elapsed | evidence |
|---|---|---:|---:|---:|---|
| auditor | opus | 9/9 | 9/9 | 1,286.4 s | Real Git fixtures and live mmcg; no sentinel retry |
| critic | opus | 5/5 | 5/5 | 316.6 s | Full suite |
| intake | sonnet | 5/5 | 5/5 | 63.9 s | Full suite |
| workflow | sonnet | 26/26 | 26/26 | 372.3 s | Full suite; includes comment-policy and architecture-risk cases |

The workflow suite includes four comment-discipline regressions: zero comments
for straightforward code, removal of narrating comments, preservation of one
non-obvious security rationale, and enforcement through the executor workflow
without explicitly invoking the standalone skill.

The workflow suite also includes four architecture-review regressions: context
loss across a runtime boundary, mutation of derived state instead of the source
of truth, non-durable idempotency under concurrent retries, and a breaking
event change across mixed-version and replay windows.

## Reading the result

- Report only complete suite runs in the table. A targeted rerun can diagnose a
  case, but it does not change the suite pass rate.
- Auditor results are valid only when the mmcg index is present. Missing index
  setup is a hard failure, not a degraded pass.
- Auditor verdicts come from the structured YAML tail rather than prose.
- Fixtures must not contain comments or metadata that reveal the expected
  answer to the model.
- Results before `4c338b6` (2026-06-10) used prose verdict matching and should
  not be compared with current runs.

Run the deterministic gates and all model suites with:

```bash
bash evals/run-verified.sh --model sonnet
```

## Ablation status

The auditor ablation compares a neutral Claude + Git/read/search baseline with
the real auditor using live mmcg. It has not been run for this snapshot, so this
scorecard makes no causal uplift claim.

```bash
python evals/ablation.py --with-mastermind
```
