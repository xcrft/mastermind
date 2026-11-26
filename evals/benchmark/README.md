# Research benchmark

Use this runbook to compare research instructions, tools or reasoning effort on the same read-only tasks. You freeze the sources, run a pinned Claude or Codex CLI for every planned attempt, keep every answer and failure, and send the answers to [offline review](REVIEW.md). Preparation, export and comparison make no model calls, but `run` spends inference usage.

Run every command from the repository root. Results stay descriptive: comparisons keep `comparability.eligible: false`, `comparison_accepted: false` and `quality_uplift: null`, and importing reviews doesn't change that.

## Prerequisites

- Python 3.10+ (3.13+ on macOS), Git and a POSIX host with `waitid(WNOWAIT)`.
- A trusted, installed adapter CLI and `mmcg` binary. The harness doesn't build, download or pick a binary.
- An output directory outside the source repository. Every output path must be new, because publication never replaces a file.

## Run the whole corpus

1. Check that the corpus tasks and keys still match their pinned sources:

   ```sh
   python3 -m evals.benchmark.corpus --source-repo . --require-current
   ```

   It exits 0 when every task, key, Git object and anchor range checks out, and 2 otherwise. A failure under `--require-current` means the research source changed, so the task and key need a new review before you run.

2. Write the configuration described in [the next section](#write-the-configuration).

3. Prepare, run and export the campaign:

   ```sh
   python3 -m evals.benchmark.campaign prepare \
     --config /absolute/path/to/config.json \
     --source-repo /absolute/path/to/mastermind \
     --tool-repo /absolute/path/to/mastermind \
     --output /absolute/path/to/new-campaign --repetitions 3
   python3 -m evals.benchmark.campaign run /absolute/path/to/new-campaign
   python3 -m evals.benchmark.campaign export /absolute/path/to/new-campaign \
     --output /absolute/path/to/new-review-set
   ```

   The Claude adapter needs `campaign run ... --credential-env ANTHROPIC_API_KEY`. Codex runs with no credential option. To pause, pass `--stop-file PATH` and create that file. The run stops before the next unstarted case, and a started case always finishes. Remove the file and run the same command again to continue.

4. Import a review into every case directory of the review set, as [the review runbook](REVIEW.md) describes.

5. Compare the arms and read the result against the product targets:

   ```sh
   python3 -m evals.benchmark.campaign compare /absolute/path/to/new-review-set \
     --baseline source --candidate portable_mmcg
   python3 -m evals.benchmark.value --review-set /absolute/path/to/new-review-set \
     --baseline source --candidate portable_mmcg \
     --output /absolute/path/to/new-value-report.json
   ```

   `value` reports the targets in `value.OBJECTIVES`, for example at least 90% accepted attempts and at least 10% fewer tokens per accepted task with acceptance preserved.

| Campaign rule | Behavior |
|---|---|
| Default inventory | 4 tasks x 3 conditions x 3 repetitions = 36 attempts |
| Limits | 240 attempts per campaign, 60 per case |
| Native runtime | `mmcg` is copied and pinned before preparation, so a later Cargo build can't replace it |
| Order | Rotates within each case. Repetitions must divide evenly by the arm count |
| Resume | Completed and failed attempts are kept. Unfinished attempts can't be retried one by one |
| Invalid runtime or input | The run aborts and the remaining slots stay `not_run` |
| Export and comparison | Every planned attempt, failed and missing answers included, after case, task, key and batch bindings are checked |

Repetitions of four public tasks are still four tasks, not 36 independent samples. A model, runtime or setting change needs a new campaign.

## Write the configuration

```json
{
  "model": "exact-model-id",
  "tool_revision": "full-tool-commit-id",
  "instruction_path": "skills/workflow/mastermind-codegraph-research/SKILL.md",
  "adapter": {"path": "/absolute/path/to/adapter", "sha256": "executable-sha256", "version": "exact-version", "origin": "runtime-source"},
  "mmcg": {
    "path": "/absolute/path/to/mmcg", "sha256": "executable-sha256", "version": "exact-version",
    "source_revision": "same-full-tool-commit-id", "origin": "runtime-source",
    "index_contract": {"schema_version": "9", "extractor_contract_version": "mmcg-extractors-v10", "concept_normalization_version": "mmcg-concepts-v5"}
  },
  "limits": {"timeout_seconds": 300, "trace_bytes": 2097152, "stderr_bytes": 65536, "answer_bytes": 65536, "max_turns": 8, "max_output_tokens": 4096}
}
```

Match `index_contract` to the pinned `mmcg`. To measure completion without a time, turn or token budget, set `timeout_seconds`, `max_turns` and `max_output_tokens` to `null`. An omitted field keeps its default. Byte caps, tool timeouts and process cleanup always apply, and every condition uses the same limits. The hard ceilings are 3,600 s of wall time, 16 MiB per trace or diagnostic stream, 64 turns and 65,536 output tokens. Keep bounded and unbounded campaigns apart.

### Pin the Claude CLI

```json
"adapter": {
  "kind": "claude_cli",
  "cli": {"path": "/absolute/path/to/claude", "sha256": "executable-sha256", "version": "2.1.236", "origin": "runtime-source"},
  "reasoning_effort": "medium"
}
```

The adapter needs an API key and never falls back to OAuth or the keychain. It runs the CLI in an empty directory with no built-in tools, no saved session and one strict MCP configuration, and it checks the observed tools, permission mode, working directory and model. The fixture tests target CLI 2.1.236, so another version needs a fresh run. `claude-stream.jsonl` can hold tool inputs and results, so keep it private.

### Pin the Codex CLI

Log the CLI in with ChatGPT first, then use:

```json
"adapter": {
  "kind": "codex_cli",
  "cli": {"path": "/absolute/path/to/codex", "sha256": "executable-sha256", "version": "0.160.1", "origin": "installed-runtime"},
  "auth_home": "/absolute/path/to/.codex",
  "reasoning_effort": "max"
}
```

The adapter forces ChatGPT login, clears API keys and turns off user config, rules, hooks, memory, plugins, shell and browser tools. The frozen research MCP server is the only server. The fixture tests cover the stream of CLI 0.160.1, so pin and check any other version before you compare runs.

| Codex measurement | How to read it |
|---|---|
| Model | The requested ID. The stream doesn't report the served model |
| Context tokens | Codex reports input with cache reads and writes included, so the adapter subtracts them to get uncached input |
| Billing | `cost_usd: null`. Subscription usage is spent, API billing isn't measured |
| Turns and tool calls | CLI conversation turns, not inference rounds. Tool calls are in `diagnostics.adapter.mcp_calls` |
| `diagnostics.adapter.read_ledger` | Returned, unique and repeated source lines, failed reads and pending calls |
| `diagnostics.adapter.output_breakdown` | The reported reasoning part of output. Total output already includes it |
| `diagnostics.adapter.tool_timeline` | Tool start and completion times. Time outside the intervals includes startup and network |
| Failure | A timeout, missing usage, unknown tool or `turn.failed` stays a failed attempt |

To total the read ledgers of every planned attempt without a model call:

```sh
python3 -m evals.benchmark.retrieval_analysis /absolute/path/to/campaign \
  --output /absolute/path/to/new-retrieval-report.json
```

Failed and unrun attempts stay in the denominator, and missing ledgers stay unknown.

### Use another adapter

A generic adapter gets no arguments and runs in the source projection. Stdin is one `mastermind-research-adapter-v1` JSON request and a newline, without the rubric. Stdout must hold one `init`, optional `trace` events and one `result`:

```jsonl
{"type":"init","model":"exact-model-id","adapter_version":"exact-version"}
{"type":"trace","tool":"source_read","path":"src/service.py","start_line":1}
{"type":"result","answer":"Observed behavior at src/service.py:2.","model_error":false,"turns":1,"usage":{"input_tokens":25,"output_tokens":12,"cache_read_tokens":0,"cache_write_tokens":0},"cost_usd":0}
```

The adapter must expose only the declared tools and enforce the configured budgets. Credentials reach it only through `--credential-env` with `OPENAI_API_KEY`, `ANTHROPIC_API_KEY` or `CLAUDE_CODE_OAUTH_TOKEN`. The supervisor bounds wall time and output and cleans up the process group, but it is not an OS sandbox.

## Choose the conditions

| Default condition | Instructions | Tools |
|---|---|---|
| `source` | Common research instruction | Read, search and Git inspection |
| `portable` | Common instruction plus the pinned research skill | Same source tools |
| `portable_mmcg` | Same as `portable` | Source tools plus mmcg |

To declare your own arms, add `conditions`. Condition names don't select tools, so set `tools` on each arm:

```json
{"conditions": [
  {"id": "raw", "tools": "source", "instruction_paths": []},
  {"id": "refined", "tools": "source", "instruction_paths": ["prompts/refined.md"]}
]}
```

You can declare 2 to 8 arms. Instruction files come from `tool_revision`, join with two newlines and stay under 1 MiB in total. Referenced skills are not resolved, so list every file. The public task and review key are the same for all arms, so freeze the key before you look at answers.

| Experiment | Set on each condition | Keep identical |
|---|---|---|
| Symbol lookup | `tools: "mmcg"`, `symbol_lookup: "single"` or `"batch"`, and `agents/instructions/source-lookup.md` in both arms | Instructions, model, effort and native binary |
| Retrieval routing | `agents/instructions/source-boundaries.md` in both arms, `source-routing.md` in the candidate only | Tools, lookup, source delivery, model and effort |
| Source delivery | `tools: "mmcg"`, `source_delivery: "native_full"` or `"native_reuse"`, and `agents/instructions/source-reuse.md` in both arms | Model, effort, instructions and source scope |
| Reasoning effort | `reasoning_effort`: `low`, `medium`, `high`, `xhigh` or `max` | Tools and instruction paths |

A `batch` lookup on a binary without batch support fails instead of falling back. A missing native reader fails `source_delivery` the same way. The recorded effort is the requested one, and the served effort is unknown.

### Calibrate a role prompt or effort

Declare the axis and give every condition a `role` and an effort:

```json
{"calibration": {"axis": "role_prompt"},
 "conditions": [
    {"id": "direct", "role": "direct", "tools": "source", "reasoning_effort": "high", "instruction_paths": []},
    {"id": "researcher", "role": "researcher", "tools": "source", "reasoning_effort": "high", "instruction_paths": ["agents/subagents/mastermind-researcher.md"]}
 ]}
```

With `role_prompt`, only the role label and instruction files may differ. With `effort`, only the requested effort may differ. Preparation rejects any other change with `confounded_calibration`. `role` only labels the instructions. It doesn't start a native agent or grant its tools.

To route effort per case, add `"effort_policy": {"condition": "adaptive", "policy": "bounded_readonly_v1"}` and declare an `adaptive` condition. Only `campaign prepare` applies it. The router picks `high` for a read-only request that names one to three source files with no risk marker, and `max` otherwise. It is an English-only heuristic, so it stays experimental.

## Use the corpora

### Calibration corpus

[corpus.json](corpus.json) binds each public task to its private key and indexed source files.

| Case | Focus |
|---|---|
| `task-phase-continuity-01` | Workflow history and persistence |
| `document-evidence-boundaries-01` | Document freshness and evidence limits |
| `callees-definition-boundaries-01` | Ambiguous definitions and outgoing calls |
| `reference-removal-evidence-01` | References needed before code removal |

A maintainer reviewed these keys, and they are public, so they serve development and never held-out qualification. The corpus check needs the pinned history locally, because it never fetches.

### Qualification corpus

A qualification corpus tests a frozen candidate on tasks it was not tuned on. It uses the same schemas with `role: qualification` and keys with `status: qualification_key_source_reviewed` and `acceptance_criteria`.

```sh
python3 -m evals.benchmark.corpus --corpus /absolute/private/path/corpus.json \
  --source-repo /absolute/private/path/snapshot-repo --require-current
```

| Rule | Failure |
|---|---|
| Every case in one registry has the same role | Validation error |
| Registry, tasks, keys and outputs stay outside `--source-repo` and `--tool-repo` | `corpus_private_location` before any trial |
| Qualification cases are selected with `--case` | `invalid_selection` for `--task` or `--rubric` |
| Key status matches the corpus role | `campaign_changed` |

Review exports contain the keys, so keep them in the same private storage. Keep qualification files out of every directory a tuning agent can read, `.mastermind/` included.

## Run one task

```sh
python3 -m evals.benchmark prepare \
  --case document-evidence-boundaries-01 \
  --config /absolute/path/to/config.json \
  --source-repo /absolute/path/to/mastermind \
  --tool-repo /absolute/path/to/mastermind \
  --output /absolute/path/to/experiment
python3 -m evals.benchmark run /absolute/path/to/experiment/batch-id/trial-id
```

The default plan is 3 conditions x 3 repetitions = 9 attempts. `--repetitions` accepts 1 to 20 within the 60-attempt cap. For a custom task, pass `--task PATH --rubric PATH` instead of `--case`. The rubric must match the task ID and source revision.

Run the trials in the order of `batch.json`. Each attempt runs once, needs the previous result and gets a private HOME and temporary directory. Preparation rejects leaked answer keys, unsafe paths and a wrong, partial or stale SQLite index. Only arms with mmcg tools run the indexer.

## Fix a failed run

| Symptom | What to do |
|---|---|
| `setup_error`, `identity_mismatch`, `protocol_error`, `invocation_error` | Fix the setup or runtime, then prepare a new batch. Never retry only the failed conditions |
| `timeout`, `output_limit`, `budget_exceeded`, `model_error` | Keep the failed attempt. It counts as a failure and its partial answer stays reviewable |
| `cleanup_error` | A child process outlived the run. The kept stdout is preserved, and the attempt counts as an invocation failure |
| Crash, changed settings or failed preparation | Keep the old batch or campaign, unfinished attempts included, and prepare a new complete one |
| Output path exists | Choose a new path |

Missing telemetry stays unknown and never becomes zero cost. `quality: review_pending` on a completed attempt means it waits for an offline review.

## Qualify a candidate

Write the pre-registration before the campaign and record its SHA-256:

```json
{"kind": "mastermind-qualification-prereg", "schema_version": 1, "claim": "R",
 "baseline": "max", "candidate": "high", "reviewer": "judge", "repetitions": 6,
 "clusters": {"q-01": "repo-a", "q-02": "repo-a", "q-03": "repo-b"},
 "margin": 0.10, "t_star": 2.05, "t_upper": 1.729, "veto_material_errors": 8,
 "max_unknown_usage": 5, "max_infrastructure_failures": 3, "latency": false,
 "audit": {"reviewer": "maintainer", "false_accept_limit": 0.15, "discordance_p": 0.01}}
```

Calibrate `t_star`, `t_upper` and `veto_material_errors` by simulation for the registered design first. The input fields are listed in `qualify_calibration.FIELDS`, and `simulations` must be at least `20/alpha`. Replace the nuisance assumptions with pilot measurements before registration.

```sh
python3 -m evals.benchmark.qualify_calibration --input /private/calibration-input.json \
  --output /private/new-calibration.json
```

After every case has its reviewer assessment, apply the rule to the sealed qualification review sets:

```sh
python3 -m evals.benchmark.qualify --prereg /private/prereg.json \
  --review-set /private/stage-1-review-set --review-set /private/stage-2-review-set \
  --report /private/new-report.json --decision /private/new-decision.json
```

| Step | Rule |
|---|---|
| Inputs | Exactly the two registered conditions, even repetitions, balanced positions and one configuration across review sets. `not_run`, `unfinished`, `missing_artifacts`, `setup_error`, `input_changed`, or too many infrastructure failures give `qualify_stage_invalid` |
| Unit | Each repository in `clusters` is one test unit. Task differences are averaged within a repository, then across repositories |
| Acceptance | `R`: `mean - t_star * SE > -margin`. `Q`: lower bound above zero. Candidate material errors may exceed the baseline's by at most `veto_material_errors` |
| Tokens and latency | Per-task log ratio of geometric means, failed attempts included. `R` needs the upper bound below 1 and the ratio at most 0.90. `Q` needs the upper bound below 1.10. Latency is checked the same way only when `latency` is true |
| Unresolved outcomes | Evaluated at both bounds. A decision that differs between them is `inconclusive` |
| Decision | Acceptance fails: `reject`. Acceptance and every resource step hold: `promote`. Otherwise `experimental` |
| `--interim` | Non-binding futility check after stage 1: `reject`, `experimental` or `continue` |

The report keeps per-case records, so share only the decision file. On any error neither output is written.

## Plan resource repetitions

Before a resource screen, simulate its exact task-level sign-flip test. The command makes no model call:

```sh
python3 -m evals.benchmark.resource_power \
  --output /absolute/path/to/new-resource-power.json \
  --simulations 20000 --seed 20261015
```

It evaluates 8 tasks with 8 to 64 repetitions per arm, plus 12 and 16 tasks, for a true token ratio of 0.90, and picks the first repetition count whose Wilson lower bound reaches 80% power. The retained run is [resource-power-20261015-v2.json](../baselines/resource-power-20261015-v2.json).

## Sweep repo-map payload sizes

Before you spend model calls on header size, sweep the native payload against a retained repo-map campaign. Use the `mmcg` build that produced the original header. The sweep checks the campaign's seals, runs only the local CLI and makes no model call:

```sh
python3 -m evals.benchmark.repo_map /absolute/path/to/retained-campaign \
  --native /absolute/path/to/mmcg \
  --output /absolute/path/to/new-sweep.json
```

It tests budgets of 512, 1600, 4096 and 16384 bytes for the baseline, map-only, instructions-only and bundle arms, and recommends the smallest budget whose payloads equal those at the largest. A source, request, index or payload mismatch stops the sweep, so recover the pinned inputs instead of rewriting their seals.

## Evaluate operation relevance with source review

To compare plain lexical concept search between two `mmcg` builds, follow the [fixture runbook](../fixtures/README.md) instead. Freeze the queries and exact source anchors before retrieval. Build one native binary and one index over the frozen source, then prepare review packets in a new directory outside the snapshot:

```sh
python3 -m evals.benchmark.concept_operation prepare \
  --binary /absolute/path/to/mmcg --index /absolute/path/to/index.db \
  --source /absolute/path/to/frozen-source --suite /absolute/path/to/suite.json \
  --output-dir /absolute/path/to/new-preparation
```

| Optional input | Contract |
|---|---|
| `--hints PATH` | Maps every case ID to at most eight short hints, written from the query alone before retrieval. They broaden recall but don't change the query used for scoring, so report plain and expanded recall separately |
| `--request-contracts PATH` | Maps every case ID to `{operation, constraints}`. Each constraint has a unique `id`, an exact query quote `text` and a `kind` of `input`, `state`, `scope` or `result` |

Give the source reviewer only `packets/*.json` and the bound source files, never the suite, anchors or preparation manifest. The reviewer fills each packet's template and saves one assessment per packet filename in a new directory. Every candidate needs a judgment, `unknown` included, and a missing assessment counts as a failed query. The `operation-roles-v3` rubric and its fields are defined in the [concept search reference](../../docs/reference/concept-search.md).

```sh
python3 -m evals.benchmark.concept_operation finish \
  --prepared /absolute/path/to/new-preparation/preparation.json \
  --assessments /absolute/path/to/new-assessments \
  --output /absolute/path/to/new-operation-report.json
```

`finish` checks the source, binary, database, WAL, fixture, packet and assessment hashes and makes no model call. If inputs drift, keep the attempt and prepare again instead of rewriting a packet. Once you have looked at a qualification set it becomes development data, so author a new frozen set for the next qualification. The retained run of this procedure is the [source-witness study](../baselines/concept-witness-20261016.md).

## Test the harness

```sh
python3 -m unittest discover -s tests/evals/benchmark -t .
```

The tests use disposable files, real processes and fixture adapters. They make no model calls and say nothing about model quality.
