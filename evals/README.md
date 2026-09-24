# Behavioral evaluations

Each case checks one behavior of a shipped agent or workflow skill. A pass rate
is a regression signal, not product coverage or proof of real-world reliability.

## Suites

| Cases | Checks |
|---|---|
| [`critic.jsonl`](critic.jsonl) | Final design verdict |
| [`researcher.jsonl`](researcher.jsonl) | Source-backed facts, citations, and unknowns |
| [`auditor.jsonl`](auditor.jsonl) | Post-change verdict against a real Git fixture |
| [`intake.jsonl`](intake.jsonl) | Prompt intake decision |
| [`workflow.jsonl`](workflow.jsonl) | Required and forbidden workflow signals |

[Fixture READMEs](fixtures/) describe the planted changes.
[Scorecard](scorecard.md) records complete model-backed runs.
[Research benchmark](benchmark/README.md) is a separate, unscored comparison
workflow.

## Run

Model-backed runs need authenticated `claude` and Git. Researcher and auditor
cases also need a matching `mmcg` binary. Run from the repository root:

```bash
./evals/runner.py --case c-001-slop-rethink
./evals/runner.py --suite critic
./evals/runner.py --report /tmp/mastermind-evals.json
```

`run-verified.sh` runs repository checks, builds `mmcg`, then runs the model
suites:

```bash
bash evals/run-verified.sh --model sonnet
```

Model-backed suites are hand-run. CI runs the deterministic harness checks.

## What a pass means

- Critic cases read the final `## Verdict` section.
- Auditor cases read the structured YAML verdict. A claimed test rerun counts
  only if the tool stream shows that exact command succeeded.
- Researcher cases can require source-line citations. A valid location does
  not prove the reasoning drawn from it.
- Workflow and intake cases check deterministic output signals. They run with
  no tools in disposable directories.
- Tool, permission, telemetry, or transport failures fail the case before
  semantic checks. The runner does not use an LLM judge.

Researcher and auditor fixtures are temporary Git repositories built from
[`fixtures/`](fixtures/). The runner checks the live tool inventory and bounds
each model process. Managed Claude policy can still vary between machines.

## Add a case

Copy a nearby case in the relevant JSONL file. Keep one behavior per case and
explain the regression in `why`. Use only the assertions that behavior needs:

| Field | Use |
|---|---|
| `expect.verdict` | Final critic or auditor decision |
| `expect.contains` / `contains_any` | Required signals or equivalent wording |
| `expect.not_contains` | A forbidden affirmative claim, not a phrase that may appear in a denial |
| `expect.citations` | A source anchor that matches one line in the fixture |
| `expect.code_comments` | Comment limits when generated code is under test |

Do not put the expected answer in a fixture. For an auditor case, add a complete
after-tree under `fixtures/<name>/changes/<variant>/`; missing baseline files are
deleted. `staged_paths` can leave changes staged, unstaged, and untracked for a
pre-commit audit.

Run the focused case, then the full suite. Record only complete suite results
in the [scorecard](scorecard.md).

## Reports and comparison

`--report` writes case results, token use, runtime identity, and selected-input
identity. Infrastructure failures stay distinct from failed behavioral checks.
Tool inputs and results are not saved in the report.

`--baseline-report` compares the same model, cases, inputs, and runtime
controls. Every previously passing case must still pass; suite pass rate cannot
drop; context-token p50 and p95 must both fall. Incomparable reports fail the
gate. The checked-in critic baseline has a documented legacy capture exception.

```bash
./evals/runner.py --suite critic --model opus \
  --report /tmp/mastermind-critic.json \
  --baseline-report evals/baselines/critic-opus-pre-lean.json
```

## Auditor ablation

`ablation.py` compares a neutral Git reviewer with the shipped auditor on the
same fixture cases:

```bash
python evals/ablation.py --with-mastermind
```

The two columns use different grading rules. Treat the output as diagnostics,
not a quality-uplift score.
