# Task runtime contracts

These are the rules `run-task` applies when it approves a task, runs checks,
launches the native executor and reviewer, and retries. Spec fields and executor
reports are in [specs, reports and CI](spec-and-report.md). Every receipt here
is an unsigned local JSON file that records what ran and with which result. It
says nothing about test quality or whether the change is correct.

## Project settings

`mastermind init` writes these keys to `.mastermind/setup.json`. A key is
written only when it differs from the default, and an explicit command flag
wins for one run.

| Key | `init` flag | Default and range | Used by |
|---|---|---|---|
| `worktree_max_total_bytes` | `--worktree-max-total-bytes N` | 32 MiB, 1 B to 1 GiB | [Initial worktree](#initial-worktree-ownership), pinned at the first preflight |
| `snapshot_max_total_bytes` | `--snapshot-max-total-bytes N` | 32 MiB, 1 B to 1 GiB | History, audit and strict snapshots, pinned at every preflight |
| `exec_timeout_secs`, `exec_max_turns` | `--exec-timeout N`, `--exec-max-turns N` | 1,800 s (1-7,200), 40 turns (1-100) | [Native executor](#native-executor) |
| `review_timeout_secs`, `review_max_turns` | `--review-timeout N`, `--review-max-turns N` | 600 s (1-7,200), 30 turns (1-100) | [Native semantic reviewer](#native-semantic-reviewer) |
| `profile_budget_tokens` | `--profile-budget N` | 4,000 (256-8,000) | [Profile delivery](persona.md#profile-access) |

The snapshot budget has to cover the worktree budget plus the task's own
changes. A new snapshot value reaches an open task only through a new
`run-task <spec> --pre-only` approval, and an approval whose value differs from
its state fails with `preflight_approval_binding_mismatch`. An exhausted budget
names its setting, limit, counted bytes, the next file and the `init` flag.

## Initial worktree ownership

You can approve a task in a checkout that already has local changes. The first
successful preflight records the Git commit and a manifest of those changes in
`state.json.initial_worktree`, and keeps a copy of each changed file under
`<task>/preflight-inputs/<sha256>`. Keep these files with the task when you move
the checkout.

| Input | Contract |
|---|---|
| Earlier edits, deletions and untracked files that stay unchanged | Not part of the task |
| A new change to earlier work, including a restore | Part of the task, with normal scope and report checks |
| Symbols changed in a file that was already dirty | Compared with the kept copy |
| Retry, reset, repair or reapproval | Keeps the first manifest and Git baseline |
| Missing or changed kept copy | Blocks evidence. Restore the exact file |
| Standalone `audit-spec` and CI | Keep their commit-to-worktree scope |

| Limit | Value |
|---|---|
| Entries | 1,000 |
| Manifest metadata | 512 KiB |
| One file | 5 MiB, the indexer limit |
| All kept files | `worktree_max_total_bytes` |

## Dependency-cycle risk

Preflight checks whether the task's declared files (`touches`, `creates`,
`expected_docs`) sit in an import cycle, following indexed imports in both
directions. A spec without declared file scope falls back to the file names it
mentions. A new file that is not indexed yet has no edges.

| Boundary | Contract |
|---|---|
| Search work | 50,000 neighbor visits and include steps, 50,001 unique strings (8 MiB), 10 s and 250,000 SQLite progress ticks |
| Incomplete search, SQL failure or cancel | Preflight fails as unknown risk. Keep the full scope, run `mastermind index .` and preflight again |

## Revise an approved task

After you change an approved spec, approve it again:

```bash
mastermind run-task .mastermind/tasks/001-feature/spec.md --pre-only
```

Each successful preflight keeps the exact spec and a validation receipt under
`<task>/preflight-approvals/`, pinned by `state.json.preflight_approval_sha256`.

| Reapproval input | Contract |
|---|---|
| Unchanged `FIND` and pre-edit symbol conditions | Keep their earlier validation, so an implemented edit does not have to restore old text |
| New or changed literal, symbol, signature, caller count or scope | Needs current verification |
| Declarations, creates, verification, acceptance and strict rules | Checked in full |
| Git baseline and initial-worktree manifest | Kept |
| Approval attempt | Uses one iteration, failures included |
| Resume, `--post-only` or restored old spec bytes | Cannot skip the explicit preflight |
| Missing or changed kept approval | Blocks reuse. Restore the exact file |
| Approval chain | At most 128 records and 32 MiB of reads per check |

## Observed verification

Declare a check before preflight, then run it:

```yaml
verify:
  - cmd: cargo test --locked
    run:
      id: unit
      argv: [cargo, test, --locked]
      cwd: mcp/servers/mmcg
      timeout_secs: 300
```

```bash
mastermind verification run .mastermind/tasks/001-feature/spec.md --id unit --json
```

| Declaration | Contract |
|---|---|
| `argv` | Run directly with null stdin and the inherited environment. Shell syntax needs an explicit shell executable |
| `cmd` | Must equal the displayed argv. Simple arguments stay bare, others use single quotes with `'\''` for an apostrophe. Preflight prints the required `cmd` on a mismatch |
| Executable lookup | Absolute `PATH` entries only |
| Runs per spec | 32 |
| `id` | Unique, 1-64 ASCII letters, digits, `_` or `-` |
| Arguments | 64, 8 KiB each, 32 KiB combined, no NUL |
| `cwd` | `.` or a canonical repository-relative directory |
| `timeout_secs` | 1-3,600 |
| `inputs` | Up to 128 extra files, repository-relative whatever `cwd` is |
| Captured output | 1 MiB each for stdout and stderr |

A `verify` entry without `run` keeps the weaker reported-result contract.

| Receipt | Contract |
|---|---|
| Location | `<task>/verification/<id>.json`, or a `.verification/` directory beside the run state for a spec outside a task folder |
| Records | argv, cwd, executable hash, repository, spec, baseline, iteration, exit code, duration and output hashes. No raw output or environment values |
| New attempt | Replaces the receipt with `pending` first. A second runner for the same task is refused |
| Failure, interruption, timeout or output limit | An older success can't be reused |
| Postflight | Needs the latest receipt of every observed check to be a current success |

### Snapshot coverage and freshness

The runner snapshots everything the check could read before and after the
command and compares them. A changed bound input gives `input_changed`, and the
receipt can't satisfy checks.

| Covered | Detail |
|---|---|
| Task and Git state | Spec, HEAD, index and baseline |
| Repository files | Tracked and non-ignored untracked files outside `.mastermind/` |
| Verification kit | Every file under the spec's sibling `verify/` directory, up to 1,024 entries |
| Extra inputs | Files in `verify[].run.inputs`, ignored ones included |
| Executable | The top-level executable's bytes, up to 128 MiB |
| Tracked symlinks on Unix | The link text. The target must stay inside the repository |

| Snapshot limit | Value |
|---|---|
| Observed files | 100,000 |
| File bytes per pass | 512 MiB |
| One run, both snapshots | 2 GiB |
| Preflight file reads | 1 GiB |
| Spec / Git response / receipt | 4 MiB / 8 MiB / 128 KiB |

Dirty or untracked symlinks, submodules and conflicts are rejected before
approval. Ignored dependencies, external services and the full environment are
not covered, and a temporary change restored before the final snapshot is not
detected. Keep task check scripts in `<task>/verify/`, which is bound
automatically, and list any other ignored input in `run.inputs`, for example
`inputs: [.mastermind/checks/lock.sh]`.

### Process supervision

| Event or platform | Behavior |
|---|---|
| macOS and Linux | The runner supervises the process group and terminates it on completion, timeout or interruption |
| Ctrl-C, `kill` or a closed terminal | The group stops and the receipt is `interrupted` |
| Detached child | Not contained |
| SIGKILL of the runner | The receipt stays `pending` |
| Windows | Refused before the command starts |
| Permissions | Normal file system and network access, no OS sandbox |

## Acceptance mapping

```yaml
acceptance:
  - id: recovery
    statement: An expired token is rejected without changing the account.
    checks: [unit, integration]
```

| Declaration | Requirement |
|---|---|
| Criteria | 1-64 |
| `id` | Unique, 1-64 ASCII bytes |
| `statement` | Not empty, at most 2,048 bytes, not a placeholder |
| `checks` | 1-32 unique observed check IDs. All are required |

`mastermind acceptance status SPEC --json` reports each check as `current`,
`missing`, `pending`, `failed`, `stale` or `unavailable`. It exits 0 for
`requirements_satisfied` and 1 for `blocked` or `not_declared`, and always
reports `semantic_accuracy: unknown`. Whether the checks test the statement is
the reviewer's call.

## Native executor

`run-task --exec` repeats preflight with the original baseline, then launches
Claude Code with a task-bound prompt. Native execution and review need macOS or
Linux. On Windows they refuse before they change any task state, and
`--exec --pre-only` still works for a manual preflight.

| Stage | Contract |
|---|---|
| Controller lock | Held from preflight through postflight. Another controller is refused |
| Context | Executor role, workflow and declared paths. Up to 12 literal title terms select up to 4 document hits |
| Before launch | Rechecks code, project and document revisions, then profile sources and audience access |
| Profile audience | The project's configured profile client, or `--profile-client ID` for one run. Both need an existing read grant |
| Receipt | `invocation.json` is `pending` before launch, then records hashes, policy, byte counts and outcome. `model_use` stays `unknown` |
| Mining | The process gets `MMCG_INPUT_ORIGIN=controller`, so persona capture skips it |

| Default native policy | Contract |
|---|---|
| Tools | `Read,Edit,Write,Grep,Glob,Bash` |
| Permissions | `acceptEdits`, no blanket Bash grant. Existing native rules decide command access |
| Session persistence and Chrome | Off |
| Adapter | Claude Code 2.1.267 or later within 2.1 |
| Success | Supported version, matching initialization and one successful terminal result. Exit 0 alone is not enough. A final tool error or permission denial fails the attempt |
| Inherited MCP, hooks, authentication and model | Used as configured, not verified |

| Limit | Value |
|---|---|
| Wall time / turns | See [project settings](#project-settings) |
| Input / receipt / probe output | 128 KiB / 128 KiB / 64 KiB |
| Process output / one protocol line / hashed executable | 16 MiB / 1 MiB / 2 GiB |

File scope is audited after execution, and completion still needs the report,
verification, acceptance and review gates. [Lens](lens.md) shows the recorded
delivery next to the current [context preview](persona.md#private-lens-view).

### Guarded execution

```bash
mastermind run-task .mastermind/tasks/001-feature/spec.md \
  --exec --guarded-exec --auto-review
```

Guarded mode puts a private hook in front of each native tool call and allows
only what the approved task needs. The task needs explicit file scope and
observed `verify[].run` checks.

| Tool | Allowed |
|---|---|
| Edit, Write | Exact declared paths and the executor report. Controller paths and symlink or hardlink aliases are rejected |
| Read, Grep, Glob | Paths inside the repository |
| Bash | Only the exact `verification run` commands the controller generates |
| Other tools and subagents | Rejected |

| Guarded contract | Rule |
|---|---|
| Native settings | `dontAsk`, restricted mode, isolated settings, strict empty MCP, slash commands off |
| Hook runner | The Mastermind executable, at most 256 MiB. A larger debug build records `invocation_guard_runner_too_large`, so use `cargo build --release --manifest-path mcp/servers/mmcg/Cargo.toml` |
| Decision log | Tool name, input hash and decision. No raw arguments |
| Success | Every tool call matches an allow decision and none was denied |
| Receipt schema | [v2](../../schemas/invocation-receipt-v2.schema.json). The default executor and reviewer write v1 |

If the native hook fails, the client can fall back to its own permissions (see
[hook failure behavior](https://code.claude.com/docs/en/hooks#timeouts)). That
blocks success but cannot undo an effect. An allowed verification process runs
with normal file system and network access, and there is no OS containment
after a path check.

## Native semantic reviewer

| Invocation | Effect |
|---|---|
| `review-task run` | Records one review of a pending held task |
| `run-task --auto-review` | Also evaluates completion under the controller lock. Without `--exec` it runs no preflight, executor or checks |
| `--exec --auto-review --auto-follow-up` | At most one executor retry and one fresh review, see [one semantic follow-up](#one-semantic-follow-up) |
| Negative, unknown, failed or unresolved-history result | Stops |

| Native reviewer setting | Value |
|---|---|
| Adapter | Claude Code 2.1.290 or later within 2.1 |
| Tools | Read, Grep, Glob and StructuredOutput, with `--json-schema` set to the closed reviewer answer schema |
| Permissions and MCP | `dontAsk`, restricted mode, strict empty MCP |
| Personal profile | Not injected |
| Input | Evidence manifest, patch with file modes and a separate staged patch. Binary or oversized input is rejected |

| Limit | Value |
|---|---|
| Source inventory | 10,000 files, 4 MiB per file, 64 MiB per snapshot |
| Patch / prompt | 64 KiB / 128 KiB |
| Terminal result / receipt | 1 MiB / 256 KiB |

The reviewer writes a pending `review-invocation.json` and revokes the earlier
semantic approval before it starts. The assessment comes only from the final
result string, and the controller sets `reviewer.kind: llm` and the observed
model. A failed run is `invocation_native_structured_output_invalid`,
`invocation_native_max_turns` or `invocation_native_structured_output_retries`.
A `passed` receipt can still hold judgments that block completion.

## Automatic repair

`--exec --auto-repair` retries the executor when a fresh check fails for an
ordinary reason inside the approved scope.

| Requirement | Rule |
|---|---|
| Spec | Structured acceptance and an observed `run` for every `verify` entry |
| Receipts | Every check is a current success or a fresh nonzero exit, with at least one failure |
| Executor report | `partial` status, names the failed command, classifies defects as `implementation_defect` |
| Audit findings | Only the matching check or criterion failures |
| Iteration budget | Cumulative, earlier preflights included. 1-20, default 3. `--force-iteration` is refused |

A missing or stale receipt, scope or symbol drift, a permission failure or a
timeout stops the loop and records `run_preflight`. The executor gets check and
criterion IDs and input digests, with no suggested fix. A held audit ends repair
and moves to semantic review.

### One semantic follow-up

```bash
mastermind run-task .mastermind/tasks/001-feature/spec.md \
  --exec --auto-review --auto-follow-up --max-iterations 3
```

| Admission | Requirement |
|---|---|
| State | Held audit, successful native executor, current checks, the current native review |
| Criteria | At least one `unsatisfied` criterion with a reason and evidence, and no `unknown` |
| Other judgments | Verification quality, scope control and proportionality `satisfied`, both history decisions `no_change` |
| Budget | One semantic retry per controller run, inside the 1-20 iteration limit |
| Flags | Needs `--exec --auto-review`. Refuses `--pre-only`, `--post-only` and `--force-iteration` |

The executor gets the cited unmet criteria and digests, at most 32 KiB, with
reviewer reasons passed on as unverified data. Fresh checks, audit and a
separate review follow. A second negative review stops the task.

## Task records are parsed strictly

Mastermind refuses a task record with a repeated JSON key, a floating-point
number or an uppercase digest or ID.

| Record | Error when invalid |
|---|---|
| `state.json` | The parse error, and the task stops |
| `<task>/verification/<id>.json` | `receipt_invalid` |
| Executor invocation receipt | `invocation_receipt_invalid` |
| `review-invocation.json` | `review_invocation_receipt_invalid` |
| Preflight approval | `preflight_approval_invalid` |
| `review-task submit --report` file | `semantic_review_report_invalid` with line and column |
| Guarded decision log | `invocation_guard_invalid_json` |

## Older task state

Finish any other older task with the release that started it, or start a new one.

| State | Behavior |
|---|---|
| `state.json` without `format_version` | Read as format 1, the current format |
| Same, with an `executor-report.md` or `audit.md` beside a spec not named `spec.md` | Refused. The error lists one `mv` or `rm` command per old file |
| Newer `format_version`, manifest or approval | Refused. Finish open tasks before you downgrade |
| No initial-worktree manifest | Audited from commit to worktree |
| Manifest `schema_version` 1 | Fails with `initial_worktree_schema_outdated` |
| Approval receipt `schema_version` 1 | Valid for its own audit. Reapproval fails with `preflight_approval_schema_outdated` |
| No kept approval files | `--pre-only` keeps earlier conditions only when the spec matches the saved hash, labelled `legacy_prior_approval` |
| Review receipt schema v1 | Checked against its own policy |
| `mode: lite` or `mode: standard` | Still accepted |
| Task without a structured review | Completes from a bound audit and `history-review.md` decisions marked `updated` or `not applicable` with a reason |
