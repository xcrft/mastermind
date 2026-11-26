# Behavioral evaluations

These suites run the shipped roles and workflow skills on planted cases through the Claude CLI and grade the answers with deterministic checks, without an LLM judge. Runs spend inference usage on your Claude login, so they stay outside ordinary CI.

| Suite | Cases | Checks |
|---|---|---|
| `critic` | [critic.jsonl](cases/critic.jsonl) | Final `## Verdict` and explicit uncertainty |
| `researcher` | [researcher.jsonl](cases/researcher.jsonl) | Source citations, graph use and limits |
| `auditor` | [auditor.jsonl](cases/auditor.jsonl) | Held, Drift or Broken in the YAML verdict |
| `intake` | [intake.jsonl](cases/intake.jsonl) | Refine, pass through or ask |
| `workflow` | [workflow.jsonl](cases/workflow.jsonl) | Required behavior of a workflow skill |
| `selection` | [selection.jsonl](cases/selection.jsonl) | The skill a model picks from the [skill list](../selection/README.md) |

You need the `claude` CLI logged in and `git` on PATH. The researcher and auditor suites also need a matching `mmcg` binary. Each fixture under [fixtures/](fixtures/) has a README that describes its planted change.

## Run the suites

From the repository root, `just evals --model sonnet` runs the repository gates and then every suite. To run one suite and compare it with an earlier complete report:

```sh
python3 -m evals.behavior.runner --suite critic --model opus \
  --report /tmp/mastermind-critic.json
python3 -m evals.behavior.runner --suite critic --model opus \
  --report /tmp/mastermind-critic-current.json \
  --baseline-report /tmp/mastermind-critic.json
```

 For diagnosis, add `--case ID`, `--keep-fixtures` to keep the temporary corpus, or `--verbose-failures`. A rerun of one case doesn't replace a complete suite result. A case fails on missing evidence, permission denials, transport errors or incomplete telemetry. A claimed check counts only with its exact command and an observed successful result.

## Add or change a case

1. Put one falsifiable behavior in a JSONL case and explain it in `why`.
2. For tool behavior, use a source fixture with an explicit positive oracle. Keep expected answers out of the files the model sees.
3. For the auditor, put the complete after-tree under `fixtures/<name>/changes/`. Missing baseline files are deleted, and `staged_paths` leaves only the listed paths staged.
4. Run `python3 -m unittest discover -s tests/evals/behavior -t .`, then the affected case and the complete suite.

| Case field | Use |
|---|---|
| `expect.verdict` | Final critic or auditor decision |
| `expect.contains`, `contains_any` | Required wording or equivalent wording |
| `expect.not_contains` | A forbidden affirmative claim |
| `expect.citations` | A source anchor that matches one fixture line |
| `expect.code_comments` | Comment limits for generated code |

The loader rejects unknown fields, contradictory expectations, invalid budgets and ambiguous anchors. Phrase checks can fail on a correct paraphrase, and a forbidden phrase can appear inside a denial. Diagnose such a failure against the case's intent, and don't widen the case just to pass the answer that failed.

## Compare with a plain reviewer

```sh
python3 -m evals.behavior.ablation --with-mastermind
```

This runs a plain `claude -p` reviewer and the shipped auditor on the planted-defect auditor fixtures. The two columns use different grading rules, so don't subtract them. For a fair quality comparison, use the [paired research comparison](../benchmark/REVIEW.md#compare-paired-outcomes), which grades all arms with one rubric.
