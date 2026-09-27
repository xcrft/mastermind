# Evaluation

Use deterministic checks for protocol behavior and model-backed trials for
answer quality. Every published result identifies its source, inputs and runtime.

## Choose a check

| Question | Evaluation | Result |
|---|---|---|
| Can an iteration complete without required evidence? | [Control loop](control-loop.md) | Finite guard model and selected production CLI regressions |
| Do hook intake and profile boundaries hold? | [Control loop](control-loop.md#connection-to-the-implementation) | Fixture checks for binding, influence, worker lifecycle, readiness and private UI metadata |
| Does the prompt refiner preserve intent? | [Hook intake runner](control-loop.md#refiner-protocol-and-model-evaluation) | Production parser plus retained processor outputs, semantic quality needs independent review |
| Are persona measurements reproducible? | [Persona replay](../docs/reference/persona-mining-contract.md) | Frozen Git attribution, measured diff accounting and cache consistency |
| Does a shipped role follow its instructions? | `runner.py` | Focused model-backed behavioral cases |
| Does retrieval improve a research answer? | [Research benchmark](benchmark/README.md) | Matched conditions, retained answers and offline assessments |
| What does indexing cost? | [Index benchmark](../docs/benchmarks.md) | Cold, unchanged and incremental time and memory |
| What has actually been measured? | [Scorecard](scorecard.md) | Current deterministic evidence and dated historical observations |

Run deterministic checks without a model:

```sh
just eval-harness
python3 -m evals.control_loop --output .mastermind/research/control-loop/run-01
python3 -m evals.benchmark_corpus --source-repo .
```

Run the control-loop command after sources stop changing. It requires Rust, Git
and Node on POSIX, runs real CLI tests and one aggregate Lens DOM/static suite.
The Python harness tests use fixtures. Neither command calls a model.

## Behavioral suites

| Suite | Cases | Target | Main observation |
|---|---|---|---|
| `critic` | [critic.jsonl](critic.jsonl) | Design review | Final `## Verdict` and explicit uncertainty |
| `researcher` | [researcher.jsonl](researcher.jsonl) | Code and document research | Source citations, graph use and limits |
| `auditor` | [auditor.jsonl](auditor.jsonl) | Postflight review | Held, Drift or Broken in the structured YAML verdict |
| `intake` | [intake.jsonl](intake.jsonl) | Goal refinement | Refine, pass through or ask |
| `workflow` | [workflow.jsonl](workflow.jsonl) | Portable workflow skills | Required behavior in a focused scenario |

[Hook intake cases](hook-intake.jsonl) use the separate `evals.hook_intake` runner:

```sh
python3 -m evals.hook_intake --binary /absolute/path/to/mmcg \
  --processor /absolute/path/to/protocol-processor \
  --output /private/new-report-directory
```

It invokes the production parser with task admission disabled and retains every
attempt. The processor is explicit, labels stay hidden from it, and failed
attempts stay in the denominator. See the [bounds and output contract](control-loop.md#refiner-protocol-and-model-evaluation).
No model-quality result is published for this corpus. Rust's
`persona_hooks_refiner_cli` checks protocol and lifecycle with deterministic
processors.

| Hook corpus | Cases |
|---|---:|
| Activate Mastermind | 10 |
| Continue an explicitly bound task | 7 |
| Ordinary request, including quoted instructions and negation | 15 |
| Unclear reference or missing task binding | 8 |
| Total | 40 |

The corpus covers Russian, English, mixed Russian/English, Spanish, French,
German and Chinese. Fake-processor routing tests establish protocol behavior,
not multilingual classification accuracy or preservation of meaning. A model
benchmark still needs independently reviewed labels, held-out cases and retained
outputs for every attempted case.

[Fixture READMEs](fixtures/) describe the planted changes. Model-backed researcher
and auditor runs need a matching `mmcg` binary. The runner uses deterministic
grading, without an LLM judge.

Model-backed runs require an authenticated Claude CLI. They consume inference
usage and are run explicitly, outside ordinary CI:

```sh
# Run repository gates, then all behavioral suites.
bash evals/run-verified.sh --model sonnet

# Run one suite and retain the complete report.
python3 evals/runner.py --suite critic --model opus \
  --report /tmp/mastermind-critic.json

# Compare with a matching baseline.
python3 evals/runner.py --suite critic --model opus \
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

## Measurement contract

| Direction | Metric | Interpretation |
|---|---|---|
| Useful completion | Accepted tasks / all attempted tasks, plus user corrections | Independently judged outcomes, including failed attempts |
| Reasoning quality | Material false conclusions, required evidence coverage, appropriate abstention | Review complete retained answers against a fixed source key |
| Control | Completion without required proof, stale-evidence rejection, budget stops | Per-scenario results with failures, no coverage percentage inferred from counts |
| Context efficiency | Input + cache creation + cache read tokens, p50/p95 | Preserve quality and compare the same tasks, runtime and budgets |
| Execution cost | Total tokens, elapsed time, tool rounds, retries, reported cost | All attempts counted, setup separate, missing values remain unknown |
| Persona quality | Supported habits, contradictions, review/abstention rates, held-out task benefit | Git statistics and extraction volume do not establish human traits |
| Indexing | Cold/warm/changed latency, memory, indexed/failed files | Describe corpus, machine, extraction contract and correctness checks |

For a comparable measured cost `C`, savings are `1 - C_current / C_baseline`.
Do not calculate a savings percentage when the baseline is zero, missing or
incomparable. A reduction is useful only when the required quality and control
criteria still hold. Repeated matched runs are needed to estimate variance.

| Comparison rule | Reason |
|---|---|
| Retain raw cases and recompute aggregates | Prevent selective result reporting |
| Count input + cache creation + cache read tokens | Measure context delivered to the model |
| Keep reported billing cost separate | Token volume is not a billing estimate |
| Match case/source digests, model, CLI and runtime controls | Compare equivalent experiments |
| Reject lower pass rate or regression of a previously passing case | Preserve the required quality gate |
| Require lower context-token p50 and p95 | Apply the runner's declared efficiency gate |
| Use complete current evidence for new baselines | The retained critic baseline alone has a legacy capture exception |

Research reviews retain claim support, required-known coverage, unknown handling
and reviewer disagreement. Causal uplift and automatic semantic scores remain
unmeasured.

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
python3 evals/ablation.py --with-mastermind
```

This compares a neutral Git reviewer with the shipped auditor on the same
fixtures. The columns use different grading rules, so the output is diagnostic
and cannot establish quality uplift.
