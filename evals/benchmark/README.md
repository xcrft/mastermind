# Research benchmark

This workflow prepares the same research task under three tool conditions and
retains full answers for manual review. It has no accepted quality score.
The built-in Claude adapter has deterministic tests but has not been validated
against a live model.

## Conditions

| Condition | Instructions | Tools |
|---|---|---|
| `source` | Neutral research instruction | Source read, search, Git |
| `portable` | Neutral instruction plus shipped research skill | Same source tools |
| `portable_mmcg` | Same instructions as `portable` | Source tools plus mmcg |

The public task, pinned source, private rubric, model, adapter, and budgets are
shared. Condition identity adds the instruction and graph runtime. Preparation
copies only allowlisted files from pinned Git objects into a one-commit
repository. Only `portable_mmcg` builds an index.

## Calibration corpus

[`corpus.json`](corpus.json) binds each public task to a private review key and
its indexed source files.

| Case | Focus |
|---|---|
| `task-phase-continuity-01` | Workflow history and persistence |
| `document-evidence-boundaries-01` | Document freshness and evidence limits |
| `callees-definition-boundaries-01` | Ambiguous definitions and outgoing calls |
| `reference-removal-evidence-01` | References needed before code removal |

These are published calibration cases, not held-out or representative tasks.
Their keys were source-reviewed; that does not grade a model answer. Check the
pinned files and anchors without invoking a model:

```bash
python3 -m evals.benchmark_corpus --source-repo .
```

The checker needs the pinned commits in local Git history. It does not fetch
them.

## Prepare and run

Use Python 3.10+, Git, a trusted adapter, and a matching `mmcg` executable on
POSIX. Put output outside the source repository. A config pins the model,
adapter, tool revision, graph runtime, and budgets:

```json
{
  "model": "exact-model-id",
  "tool_revision": "full-tool-commit-id",
  "instruction_path": "skills/workflow/mastermind-codegraph-research/SKILL.md",
  "adapter": {
    "path": "/absolute/path/to/trusted-adapter",
    "sha256": "sha256-of-executable",
    "version": "exact-version",
    "origin": "runtime-origin"
  },
  "mmcg": {
    "path": "/absolute/path/to/mmcg",
    "sha256": "sha256-of-executable",
    "version": "exact-version",
    "source_revision": "same-full-tool-commit-id",
    "origin": "runtime-origin",
    "index_contract": {
      "schema_version": "8",
      "extractor_contract_version": "mmcg-extractors-v6",
      "concept_normalization_version": "mmcg-concepts-v2"
    }
  },
  "limits": {
    "timeout_seconds": 300,
    "trace_bytes": 2097152,
    "stderr_bytes": 65536,
    "answer_bytes": 65536,
    "max_turns": 8,
    "max_output_tokens": 4096
  }
}
```

```bash
python3 evals/benchmark.py prepare \
  --case document-evidence-boundaries-01 \
  --config /absolute/path/to/config.json \
  --source-repo /absolute/path/to/mastermind \
  --tool-repo /absolute/path/to/mastermind \
  --output /absolute/path/to/benchmark-output
```

Preparation creates nine trials by default: three repetitions with rotating
condition order. It does not call a model. Run every trial once, in the order
recorded in `batch.json`:

```bash
python3 evals/benchmark.py run /absolute/path/to/batch-id/trial-id \
  --credential-env ANTHROPIC_API_KEY
```

`--credential-env` forwards only the named credential. Accepted names are
`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and `CLAUDE_CODE_OAUTH_TOKEN`. Other
inherited environment variables are cleared. A failed or interrupted attempt
stays in the record; prepare a new batch instead of selectively retrying.

For custom tasks, use `--task PATH --rubric PATH` instead of `--case`. Declare
`mmcg.indexed_files` if some allowed source files are not indexed. Custom keys
do not inherit the corpus's source-review claim.

## Adapter contract

A generic adapter reads one JSON request from stdin and writes JSON Lines to
stdout: one `init`, optional `trace` events, then one `result`. The request
includes the public task, source root, instructions, model, limits, and allowed
tools. It never includes the private rubric or another trial.

The adapter must expose only the declared tools and honor the model and budget.
A successful result needs a nonempty answer, observed model identity, turns,
and token usage. Protocol, identity, setup, budget, and model failures remain
distinct; missing telemetry is not treated as zero.

For the built-in Claude CLI adapter, replace `adapter` in the config with:

```json
{
  "kind": "claude_cli",
  "cli": {
    "path": "/absolute/path/to/claude",
    "sha256": "sha256-of-executable",
    "version": "exact-version",
    "origin": "runtime-origin"
  }
}
```

It uses bare mode and requires an explicitly forwarded
`ANTHROPIC_API_KEY`. The source broker allows bounded reads, search, and Git;
only the third condition gets graph tools. Managed host policy can still affect
the CLI.

## Results and review

Each trial retains its manifest, request, private rubric, trace, diagnostics,
full bounded answer when available, and result. `run_status` records transport
and setup outcomes; `quality` is `review_pending` for retained answers or
`not_evaluated` otherwise. A completed process is not a correctness grade.

Use [offline answer review](REVIEW.md) to export every planned attempt and
import independent assessments. Reviews do not change the original run result
or produce an accepted uplift score.

The adapter runs as the host user. Directory separation, environment filtering,
and hashes are not an OS sandbox or proof of runtime provenance. Every result
keeps `comparability.eligible: false`; every batch keeps
`comparison_accepted: false` and `quality_uplift: null`.
