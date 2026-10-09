# Behavioral evaluations

| Suite | Cases | Target | Main observation |
|---|---|---|---|
| `critic` | [critic.jsonl](cases/critic.jsonl) | Design review | Final `## Verdict` and explicit uncertainty |
| `researcher` | [researcher.jsonl](cases/researcher.jsonl) | Code and document research | Source citations, graph use and limits |
| `auditor` | [auditor.jsonl](cases/auditor.jsonl) | Postflight review | Held, Drift or Broken in the structured YAML verdict |
| `intake` | [intake.jsonl](cases/intake.jsonl) | Goal refinement | Refine, pass through or ask |
| `workflow` | [workflow.jsonl](cases/workflow.jsonl) | Portable workflow skills | Required behavior in a focused scenario |

[Fixture READMEs](fixtures/) describe the planted changes. Model-backed researcher
and auditor runs need a matching `mmcg` binary. The runner uses deterministic
grading, without an LLM judge.

Model-backed runs require an authenticated Claude CLI. They consume inference
usage and are run explicitly, outside ordinary CI:

```sh
# Run repository gates, then all behavioral suites.
bash evals/run-verified.sh --model sonnet

# Run one suite and retain the complete report.
python3 -m evals.behavior.runner --suite critic --model opus \
  --report /tmp/mastermind-critic.json

# Compare with a matching baseline.
python3 -m evals.behavior.runner --suite critic --model opus \
  --report /tmp/mastermind-critic-current.json \
  --baseline-report evals/baselines/critic-opus-pre-lean.json
```

Use `--case ID` for diagnosis, `--keep-fixtures` to inspect a temporary corpus,
and `--verbose-failures` for detailed assertion failures. A targeted rerun does
not replace the result of a complete suite.

| Runner contract | Enforcement |
|---|---|
| Prompt-only suites | Empty tool set and a fresh temporary directory |
| Researcher/auditor suites | Shipped agent, disposable Git fixture, index and live MCP server |
| Identity | Frozen model, CLI, harness, role, case, source and tool identities |
| Claimed verification | Exact command plus observed successful result |
| Failure accounting | Missing evidence, denials, transport errors and incomplete telemetry fail the case |
| Execution bounds | Per-case wall time and output caps, process-group cleanup |
| Host boundary | Managed client policies still apply, no OS sandbox or reviewer independence claim |

## Add or change a case

1. Put one falsifiable behavior in a JSONL case and explain it in `why`.
2. Use a source fixture for tool behavior and an explicit positive oracle.
3. Keep expected answers out of source files shown to the model.
4. Prefer structured outputs, observed results and exact citation anchors.
5. Validate the harness, then run the affected case and the complete suite.

| Case field | Use |
|---|---|
| `expect.verdict` | Final critic or auditor decision |
| `expect.contains` / `contains_any` | Required signals or equivalent wording |
| `expect.not_contains` | Forbidden affirmative claim, accounting for denials |
| `expect.citations` | Source anchor matching one fixture line |
| `expect.code_comments` | Comment limits for generated code |

Auditor variants provide a complete after-tree under `fixtures/<name>/changes/`.
Missing baseline files are deleted. `staged_paths` leaves selected changes staged
and others unstaged or untracked for a pre-commit audit.

The loader rejects unknown fields, contradictory expectations, invalid budgets,
unavailable tools and ambiguous citation anchors. Auditor verdicts come from
the canonical YAML block. Source citations must name valid bounded locations.
Citation validity does not prove that a conclusion follows from the source.

Phrase assertions can fail on a correct paraphrase or pass on shallow wording.
A forbidden phrase can also occur inside a denial. Diagnose those failures
against the intended contract. Do not widen a case merely to admit the answer
that just failed. Long-task adherence and semantic accuracy need separate
outcome evaluations.

## Auditor ablation

```sh
python3 -m evals.behavior.ablation --with-mastermind
```

This compares a neutral Git reviewer with the shipped auditor on the same
fixtures. The columns use different grading rules, so the output is diagnostic
and cannot establish quality uplift. Do not subtract those scores. The
[paired research comparison](../benchmark/REVIEW.md#compare-paired-outcomes) uses
one outcome rubric for all arms. Auditor defect/no-defect quality needs its own
shared semantic key and adapter; the legacy ablation is not that experiment.
