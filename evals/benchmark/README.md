# Research benchmark transport

`evals/benchmark.py` prepares independent source snapshots and runs a pinned
executable adapter through a bounded JSON protocol. The deterministic tests use
small executable adapters, a fixture Claude CLI, a real Python MCP broker and a
SQLite-producing fixture indexer. No model calls or native builds are part of
those tests.

The built-in Claude CLI adapter is runnable with an explicitly pinned CLI and
API key. It has not been validated against a live model. This layer does not
include a semantic grader or a measured quality baseline. Every result
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

The private rubric must name the public task's `id` as `task_id` and its exact
`revision` as `source_revision`. An absent or different rubric revision is a
configuration error, so updating the task snapshot cannot silently reuse an old
answer key. The checked-in calibration key has received independent agent source
review, including Held/Drift/Broken outcomes and persistence failure boundaries.
This does not establish runtime behavior or grade a model answer. Changing a task
or key requires fresh trials; their full contents participate in trial identity.

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

For the generic adapter, the pinned executable receives no arguments. Its working directory is the
allowlisted source projection; stdin contains one JSON object followed by a
newline. `request.json` holds that object:

- `protocol: mastermind-research-adapter-v1`;
- public `task`, `source_root` and file hashes;
- the synthetic `projection_revision` in manifest version 2;
- `system_instruction`, `portable_instruction`, `model`, `limits`;
- `available_tools`: `source_read`, `source_search`, `source_git`, and only in
  condition three, `mmcg` with its binary and index paths. Version 2 also includes
  the pinned native runtime, index hash, contracts and indexed-file inventory.

Previously prepared version 1 generic requests retain their original shape and
remain runnable. Newly prepared trials use manifest version 2.

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

A failed adapter may emit `failure: {"state": "setup_error", "code": "reason"}`
alongside an empty answer and `model_error: false`. Allowed failure states are
`setup_error`, `protocol_error`, `identity_mismatch`, `timeout`, `output_limit`,
`invocation_error`, `model_error` and `budget_exceeded`. Only `model_error` uses
`model_error: true`. `init.model` may be null when setup fails before observing a
model. Missing telemetry stays unknown; it is not fabricated for failed runs.

The supervisor enforces wall time and stdout/stderr byte caps. It retains a
complete final answer only if it fits the answer byte cap. It kills its own
process group on timeout, output overflow or completion. Token/turn enforcement
is the adapter's responsibility; the harness reports exceedances separately.

## Built-in Claude CLI adapter

Replace the config's `adapter` object with:

```json
{
  "kind": "claude_cli",
  "cli": {
    "path": "/absolute/path/to/claude",
    "sha256": "sha256-of-that-executable",
    "version": "2.1.236",
    "origin": "how-this-runtime-was-obtained"
  }
}
```

The command/stream contract was checked against CLI 2.1.236 and the official
[CLI reference](https://code.claude.com/docs/en/cli-reference),
[headless guide](https://code.claude.com/docs/en/headless) and
[SDK message types](https://code.claude.com/docs/en/agent-sdk/typescript).
Deterministic subprocess tests exercise that contract with a fixture executable;
they do not establish compatibility with another installed version or a live API.

Preparation copies the adapter and its Python modules into each trial and pins
every file, the preparation Python executable and the declared Claude binary.
Python uses `-I -S -B` to disable user/site imports and bytecode writes. Verification
checks the bundle, sidecar descriptor and executable hashes before and after
invocation. The CLI's `--version` output and stream version must match the pin.
This does not attest the interpreter's shared libraries or CLI provenance.

The CLI runs in a new empty `client/` directory with fresh HOME/XDG state, bare
mode, an explicit system prompt, no built-in tools, no slash commands, empty
setting sources, no session persistence and one strict MCP configuration. The
adapter checks the observed tool inventory, server connection, permission mode,
extensions, working directory and model identities. Managed host policies can
still affect execution; separate directories are not an OS sandbox.

Bare mode requires an explicitly forwarded `ANTHROPIC_API_KEY`:

```bash
python3 evals/benchmark.py run /absolute/path/to/batch-id/trial-id \
  --credential-env ANTHROPIC_API_KEY
```

The adapter does not read OAuth/keychain credentials or fall back to another
auth mode. The broker's environment blanks credential variables, and the native
server receives a fresh environment without credentials. The model prompt
contains only the public task fields and supplied instructions.

The broker exposes UTF-8 source ranges of at most 200 lines, bounded literal
search and a frozen Git view with `files`, `log` and `show`. Every path must be in
the source allowlist; each directory component is opened without following
symlinks, and the broker retains an immutable source snapshot. Empty files and
form-feed characters preserve file/AST line numbering.

Only condition three also exposes `mmcg_concept`, `mmcg_search`, `mmcg_outline`,
`mmcg_files`, `mmcg_callers` and `mmcg_callees`. Arguments cannot change the root,
index, command, environment or SQL. `mmcg_files.prefix` rejects `%`, `_` and
backslash because the native implementation treats them as LIKE metacharacters.
The broker invokes the pinned `--index FILE serve` read-only snapshot path,
checks index contracts and hashes, and preserves native `isError`, ambiguity,
freshness, precision notes and truncation. A failed transport is closed before
the next query. Tool replies are capped at 64 KiB and calls at 128 per server.

The CLI receives `--max-turns`. Output tokens have a per-response CLI cap and a
live stop when observed cumulative message usage exceeds the trial budget; this
is **not an exact aggregate token cap** and can overshoot. Repeated cumulative
updates are counted once. A token-cutoff answer, model switch, permission denial
or missing success telemetry cannot become `completed`.

`claude-stream.jsonl` retains bounded raw events as they arrive, including tool
inputs/results, while `trace.jsonl` records normalized tool calls and the terminal
envelope. Wall time covers version checking and model invocation; the outer
supervisor also kills nested CLI/MCP processes when the adapter is interrupted.
These are resource and protocol controls over a trusted CLI, not a host sandbox.

## Results and review

Each trial retains a frozen `manifest.json`, public `request.json`, private
`rubric.json`, bounded `trace.jsonl` and `stderr.txt`, complete bounded
`answer.md` when present, and `result.json`. Indexing output and elapsed setup
time are recorded separately from the adapter's investigation time.

| Result field | Meaning |
|---|---|
| `run_status` | Setup, timeout, output cap, model budget, invocation, protocol, model, identity or input-mutation failure; otherwise `completed` |
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
the pinned historical commit. Its source-reviewed private key is not a
representative or held-out quality benchmark. Before drawing
conclusions, validate the adapter with a live API and enforced host isolation and
verified runtime provenance, independently review additional tasks and keys,
run all conditions, then review blinded final answers against the same rubric.

## Deterministic checks

```bash
python3 -m unittest evals/test_benchmark.py evals/test_claude_adapter.py
```

The tests use real subprocess I/O, disposable Git histories and SQLite indexes.
They cover source projection, configuration separation, input identity checks,
failed and partial indexes, full answer retention, timeout and process cleanup,
malformed protocol events, telemetry separation and counterbalanced preparation.
The Claude tests also cover actual source/MCP subprocesses, graph result fidelity,
transport recovery, credentials, pinned bundle tampering, model switches, live
budget stops, partial stream retention and nested process cleanup.
