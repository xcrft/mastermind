# Research benchmark transport

`evals/benchmark.py` prepares independent source snapshots and runs a pinned
executable adapter through a bounded JSON protocol. The deterministic tests use
small executable adapters and a SQLite-producing indexer. No model calls or
native builds are part of those tests.

This is the preparation and transport layer. It does not include a production
model adapter, a semantic grader, or a measured quality baseline. Every result
has `comparability.eligible: false`; every batch has
`comparison_accepted: false` and `quality_uplift: null`.

## Three conditions

| Condition | Instructions | Exposed tool contract |
|---|---|---|
| `source` | Common neutral research instruction | Read, search, Git source inspection |
| `portable` | Neutral instruction plus pinned shipped research skill | Same source tools |
| `portable_mmcg` | Exactly the same instructions as `portable` | Same source tools plus mmcg |

The public task, source revision and file bytes/modes, hidden rubric, exact
requested model, adapter identity and budgets share a `common_sha256`.
`condition_sha256` adds the condition, instruction bytes and indexer identity,
including index contracts and declared indexed files. Each trial also hashes
its actual adapter request, including its unique paths.

Preparation reads exact Git objects from an explicit file allowlist. It creates
a new repository with one synthetic commit. The original repository's objects,
history, local configuration and omitted files are not copied. Allowed blobs
keep their original bytes even when supplied `.gitignore` or `.gitattributes`
files would change a normal `git add`. Symlinks, submodules, traversal paths,
client configuration directories and benchmark answer keys under `evals/` are
excluded. Product documentation can be included explicitly.

Only `portable_mmcg` runs the configured indexer. Setup rejects a nonzero exit,
partial or invalid SQLite database, nonempty WAL/journal, wrong schema/root, or
an indexed-file inventory whose hashes differ from the declared source subset.
The subset must account for what that mmcg version indexes; Markdown documents
remain available to source tools even when they are not indexed.

## Prepare trials

Requires Python 3.10+ and Git on a POSIX host. Choose an output directory outside
the source repository. Supply a trusted adapter implementing the protocol below
and an already available matching mmcg executable. The runner never builds,
downloads or falls back to a binary from PATH.

Create a local config using actual values in place of these placeholders:

```json
{
  "model": "exact-model-id-observed-by-the-adapter",
  "tool_revision": "full-lowercase-tool-commit-id",
  "instruction_path": "skills/workflow/mastermind-codegraph-research/SKILL.md",
  "adapter": {
    "path": "/absolute/path/to/trusted-adapter",
    "sha256": "sha256-of-that-executable",
    "version": "exact-adapter-version",
    "origin": "how-this-runtime-was-obtained"
  },
  "mmcg": {
    "path": "/absolute/path/to/mmcg",
    "sha256": "sha256-of-that-executable",
    "version": "exact-mmcg-version",
    "source_revision": "same-full-tool-commit-id",
    "origin": "how-this-runtime-was-obtained",
    "index_contract": {
      "schema_version": "8",
      "extractor_contract_version": "mmcg-extractors-v6",
      "concept_normalization_version": "mmcg-concepts-v2"
    },
    "indexed_files": [
      "mcp/servers/mmcg/src/run_task.rs",
      "mcp/servers/mmcg/src/verify_spec.rs",
      "mcp/servers/mmcg/src/audit_spec.rs"
    ]
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

The task's source revision and the tool/instruction revision are separate. The
included calibration task investigates an old, fixed repository commit. Pin the
instruction and mmcg source together without changing the task revision.

```bash
python3 evals/benchmark.py prepare \
  --task evals/benchmark/tasks/task-phase-continuity-01.json \
  --rubric evals/benchmark/rubrics/task-phase-continuity-01.json \
  --config /absolute/path/to/local-config.json \
  --source-repo /absolute/path/to/mastermind \
  --tool-repo /absolute/path/to/mastermind \
  --output /absolute/path/to/benchmark-output
```

This creates nine trials by default: three repetitions with rotating condition
order. `batch.json` records the planned sequence, including setup failures.
Preparation never invokes the model adapter. A failed index setup remains a
failed graph trial; it cannot become a source-only result.

Run prepared trials in the recorded order, retaining all planned attempts:

```bash
python3 evals/benchmark.py run /absolute/path/to/batch-id/trial-id
```

`--credential-env OPENAI_API_KEY` explicitly forwards that credential from the
invoking environment. The other accepted names are `ANTHROPIC_API_KEY` and
`CLAUDE_CODE_OAUTH_TOKEN`. Credentials are not written into the request or
manifest. All other inherited environment variables are cleared. Each trial
gets fresh HOME, XDG and temporary directories. Every attempt takes an exclusive
lock; a failed trial cannot be silently rerun. Prepare a new balanced batch when
the runtime or experiment configuration changes.

## Adapter protocol

The pinned executable receives no arguments. Its working directory is the
allowlisted source projection; stdin contains one JSON object followed by a
newline. `request.json` holds that object:

- `protocol: mastermind-research-adapter-v1`;
- public `task`, `source_root` and file hashes;
- `system_instruction`, `portable_instruction`, `model`, `limits`;
- `available_tools`: `source_read`, `source_search`, `source_git`, and only in
  condition three, `mmcg` with its binary and index paths.

The request does not contain the rubric, expected conclusions, other trials, or
the original repository path. It describes a read-only tool contract. The
adapter must expose only these tools, honor the model/turn/output budgets,
disable unrelated settings, plugins, hooks, memory and conversation history,
and preserve the supplied instructions. The portable skill's embedded
references are not resolved by the harness; the adapter receives its exact
file bytes. Do not substitute researcher-agent frontmatter with a different
model or mandatory graph tools.

stdout is UTF-8 JSON Lines, with exactly one `init`, zero or more `trace` events,
then exactly one terminal `result`. stderr is a separate bounded diagnostic
stream. Non-JSON logging on stdout is a protocol error.

```jsonl
{"type":"init","model":"exact-model-id","adapter_version":"exact-adapter-version"}
{"type":"trace","tool":"source_read","path":"src/service.py","start_line":1}
{"type":"result","answer":"Observed behavior at src/service.py:2.","model_error":false,"turns":1,"usage":{"input_tokens":25,"output_tokens":12,"cache_read_tokens":0,"cache_write_tokens":0},"cost_usd":0}
```

`answer` must be a string and nonempty on success. An adapter-reported model
failure uses `model_error: true`; it may preserve a partial answer. Required
telemetry is nonnegative integer token counts and a positive integer turn count.
Cost is optional; absent or invalid cost is unknown, never inferred to be zero.
The observed model and adapter version must match the manifest exactly.

The supervisor enforces wall time and stdout/stderr byte caps. It retains a
complete final answer only if it fits the answer byte cap. It kills its own
process group on timeout, output overflow or completion. Token/turn enforcement
is the adapter's responsibility; the harness reports exceedances separately.

## Results and review

Each trial retains a frozen `manifest.json`, public `request.json`, private
`rubric.json`, bounded `trace.jsonl` and `stderr.txt`, complete bounded
`answer.md` when present, and `result.json`. Indexing output and elapsed setup
time are recorded separately from the adapter's investigation time.

| Result field | Meaning |
|---|---|
| `run_status` | Setup, timeout, output cap, invocation, protocol, model, identity or input-mutation failure; otherwise `completed` |
| `quality` | `not_evaluated`, or `review_pending` when an answer is retained; score is always null |
| `diagnostics` | Usage completeness, reported tools, budget exceedances, elapsed time, exit code and protocol issues |
| `comparability` | Always ineligible in this transport slice; records additional failure reasons |

Missing usage or an unexpected reported tool does not become an incorrect
research finding. Conversely, a successful process or valid citation syntax
does not prove the answer's reasoning. A valid partial answer can remain
available for review even when invocation or input verification fails.

The generic adapter runs as the host user. Separate directories, filtered
environment, read-only file modes and hash checks are **not an OS sandbox**: an
adapter could read sibling rubrics, escape its process group or access unrelated
host files. Executable hashes detect mismatched bytes; supplied source revision
and origin are declarations, not verified attestations. These limitations are
recorded in every result and prevent any accepted quality comparison.

The one checked-in task is a calibration of source reading and uncertainty at
the pinned historical commit. Its private key still requires semantic review;
it is not a representative or held-out quality benchmark. Before drawing
conclusions, add a production adapter with enforced tool/file isolation and
verified runtime provenance, independently review additional tasks and keys,
run all conditions, then review blinded final answers against the same rubric.

## Deterministic checks

```bash
python3 -m unittest evals/test_benchmark.py
```

The tests use real subprocess I/O, disposable Git histories and SQLite indexes.
They cover source projection, configuration separation, input identity checks,
failed and partial indexes, full answer retention, timeout and process cleanup,
malformed protocol events, telemetry separation and counterbalanced preparation.
