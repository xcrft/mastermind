# Task runtime contracts

This page holds the exact rules `run-task` applies when it approves a task, runs
its checks, launches the native executor and reviewer, and retries. The
[workflow guide](../workflow.md) shows the order a person follows, and
[specs, reports and CI](spec-and-report.md) covers the spec fields and the
executor report.

Every receipt on this page is a local JSON file the repository owner can edit.
A receipt shows what ran, with which inputs and which result. It is not signed,
and it says nothing about test quality or whether the change is correct.

## Initial worktree ownership

You can approve a task in a checkout that already has local changes. The first
successful preflight records the original Git commit and a manifest of those
changes in `state.json.initial_worktree`. It also keeps a copy of each changed
regular file as it was before the task, under `<task>/preflight-inputs/<sha256>`.
Keep these files with the task when you move or recover the checkout.

| Input or transition | Contract |
|---|---|
| Pre-existing tracked edits, deletions and untracked files that stay unchanged | Not part of the task's file set |
| A new change to prior work, including a restore to the commit contents | Part of the task. Normal scope and report checks apply |
| Symbols added or removed in a file that was already dirty | Compared with the retained copy |
| Retry, reset, repair or reapproval | Keeps the first manifest and the original Git baseline |
| Missing or changed retained copy | Blocks evidence. Restore the exact retained file |
| Standalone `audit-spec` and CI | Keep their commit-to-worktree scope |
| Freshness and semantic review | Still bind current execution inputs. Ownership grants no writes |

| Limit | Value |
|---|---|
| Entries | 1,000 |
| Manifest metadata | 512 KiB |
| One file | The indexer's per-file byte limit |
| All retained files | `worktree_max_total_bytes`, default 32 MiB, at most 1 GiB |

Unsupported or incomplete input fails preflight before handoff. The release
draft counts the selected task files from the original commit and lists the
files changed from the initial worktree separately.

### Byte budgets

Two project settings in `.mastermind/setup.json` bound how much a task reads.
Set them with `mastermind init`.

| Setting | `init` flag | Default and range | Pinned when |
|---|---|---|---|
| `worktree_max_total_bytes` | `--worktree-max-total-bytes N` | 32 MiB, 1 B to 1 GiB | The first preflight, kept for the life of the task |
| `snapshot_max_total_bytes` | `--snapshot-max-total-bytes N` | 32 MiB, 1 B to 1 GiB | Every preflight (`--pre-only`, `--reset`, `--exec` and the native loop) |

The snapshot budget covers the history input and audit snapshots and the strict
workflow snapshot, in postflight, review, close, policy evidence and resume.
The history snapshot also reads the task's prior local files, so the snapshot
budget has to cover the worktree budget plus the task's own changes. If you
raise one, you usually raise both.

To apply a new snapshot value to an open task, change the setting and run
`mastermind run-task <spec> --pre-only`. That pins the value as a new approval,
so audit and review have to run again. An approval whose value differs from its
state is rejected with `preflight_approval_binding_mismatch`. A budget that runs
out is reported with its setting, limit, counted bytes, the next file and the
`init` flag that raises it.

## Dependency-cycle risk

Preflight checks whether the task's files sit in an import cycle. It reads the
normalized `touches`, `creates` and `expected_docs` paths. A spec without
declared file scope falls back to the file names it mentions. Malformed
frontmatter or an invalid path fails the risk check, and body references never
widen a declared scope.

For each task file, the search follows indexed imports in both directions until
it finds a path that leaves the file and comes back to it, or runs out of
imports in one direction. A path may pass through files outside the task. A new
file that is not indexed yet has no import edges.

| Boundary | Contract |
|---|---|
| Shared search work | 50,000 neighbor visits, raw C++ includes and include resolution steps |
| Retained paths and C++ lookup strings | 50,001 unique strings and 8 MiB of UTF-8 |
| Query budget | 10 seconds and 250,000 SQLite progress ticks, or a tighter caller budget |
| Incomplete search | Partial membership is dropped and preflight fails as unknown risk |
| SQL failure or cancellation | Preflight fails. An empty risk report is never approved |

The result covers syntactic indexed imports, so a runtime cycle or a stale index
can still hide. C++ suffix resolution can match several candidate headers when
include roots are unknown. The whole-graph dependency-cycle query has its own
50,000-pair limit.

If the search is incomplete, look at the declared paths and their indexed
dependencies, keep the complete task scope and rerun preflight. Repair an
unreadable or stale index with `mastermind index .` first.

## Revise an approved task

After you change an approved spec, approve the revised contract again:

```bash
mastermind run-task .mastermind/tasks/001-feature/spec.md --pre-only
```

Each successful preflight keeps the exact spec and a validation receipt under
`<task>/preflight-approvals/`, pinned by `state.json.preflight_approval_sha256`.
Keep these files and the initial-worktree copies with the task.

A spec can state conditions that must hold before the edit, such as a literal
text the file must contain (`FIND`) or a symbol with a given caller count. The
[spec reference](spec-and-report.md) defines them.

| Reapproval input or transition | Contract |
|---|---|
| Unchanged `FIND` and pre-edit symbol conditions | Keep their earlier successful validation. An implemented replacement does not have to restore the old text or symbol |
| Preflight again with the same retained spec | Rechecks current conditions in full. Invalid or unreadable targets still block approval |
| New or changed literal, symbol, signature, caller count, file or language scope | Needs current verification |
| Current declarations, creates, verification, acceptance and strict requirements | Checked in full against the revised contract |
| Original Git baseline and initial-worktree manifest | Kept. Implementation stays in the task delta |
| Approval attempt | Uses one iteration from the cumulative budget, failures included |
| Native invocation requirement | Kept. A receipt from an older spec or iteration can't satisfy the new attempt |
| Ordinary resume, `--post-only` or restoring old spec bytes | Can't skip the required explicit preflight |
| Missing or changed retained approval | Blocks reuse. Restore the exact file |
| Approval chain | At most 128 records and 32 MiB of retained receipt and spec reads per check |

## Observed verification

Declare a check before preflight:

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

| Execution input | Contract |
|---|---|
| `argv` | Run directly, with null stdin and the inherited environment |
| Shell syntax | Needs an explicitly declared shell executable |
| `cmd` | Must equal the displayed argv. Simple arguments are unquoted, others use single quotes, with an embedded apostrophe written as `'\''`. Preflight prints the required `cmd` on a mismatch |
| Executable lookup | A bare name is looked up in absolute `PATH` entries. Relative and empty entries are skipped |
| Declaration without `run` | Keeps the weaker reported-result contract |

| Declaration | Limit |
|---|---|
| Runs per spec | 32 |
| Run ID | Unique, 1-64 ASCII letters, digits, `_` or `-` |
| Arguments | 64, 8 KiB each, 32 KiB combined, no NUL bytes |
| `cwd` | `.` or a canonical repository-relative directory |
| Timeout | 1-3,600 seconds |
| Captured output | 1 MiB each for stdout and stderr |

| Receipt or transition | Contract |
|---|---|
| Location | `<task>/verification/<id>.json`. For a spec outside a task folder, a `.verification/` directory beside its run-state file |
| Recorded inputs | argv, cwd, resolved executable hash and the repository, spec, baseline and iteration |
| Recorded result | Exit code, duration, stream byte counts and hashes. Raw output and environment values are not stored |
| New attempt | Replaces the previous receipt with `pending` before it starts |
| Second runner for the same task | Refused |
| Failure, interruption, timeout, output limit or unfinished attempt | An older success can't be reused |
| Postflight | Needs the latest receipt of every observed check to be a current success, plus the normal executor report |

Running a check neither needs nor changes the executor report.

### Snapshot coverage and freshness

A receipt is bound to a snapshot of everything the check could read. The runner
takes the snapshot twice per pass and compares them, before and after the
command.

| Snapshot component | Coverage |
|---|---|
| Task and Git state | Spec, HEAD, index and baseline |
| Repository files | Tracked and non-ignored untracked files outside `.mastermind/` |
| Verification kit | Regular files under the spec's sibling `verify/` directory, ignored ones included |
| Extra check inputs | Files listed in `verify[].run.inputs`, ignored ones included |
| Executable | The top-level executable's bytes |
| Tracked symlinks on Unix | The link text is bound without following the link. The target must stay inside the repository and exist. Absolute, escaping, dangling and chained targets are rejected |
| Link target contents | Bound only when the target is tracked, non-ignored or an explicit input. Ignored target bytes are not read through the link |
| Dirty or untracked symlinks | Rejected before handoff. Tracked link support grants no access through aliases |
| Submodules and conflicts | Rejected before approval for observed checks |

| Input | Limit |
|---|---|
| Observed files | 100,000 |
| File bytes per pass | 512 MiB |
| File content held in memory | One 64 KiB streaming buffer |
| Preflight file reads | 1 GiB |
| One verification run, both snapshots | 2 GiB |
| Spec reads | 4 MiB |
| Git response | 8 MiB |
| Executable | 128 MiB |
| Receipt JSON | 128 KiB |
| Verification kit | 1,024 files and directories per task |
| Extra inputs | 128 unique files per check |

All reads share the operation deadline and cancellation signal. A file that
grows, shrinks, is replaced or can't be read fails the snapshot, and the error
names the file. These caps bound I/O only. The separate working-tree diff reader
keeps its 5 MiB per-file cap, so a change to a larger file can still make
`audit-spec` fail with `index_stale`.

| Change or limitation | Effect |
|---|---|
| Unsupported, over-limit or unreadable snapshot | Preflight rejects observed checks early. A direct run records `unavailable` with the reason, and keeps the exit code if the command had started |
| Changed bound input | `input_changed`, the receipt can't satisfy checks |
| Dependency edit, staging, commit, executable replacement or new preflight | Can make the receipt stale |
| Receipt change during an unfinished review | Invalidates that review |
| Tracked context update | Can require new checks and audit |
| Ignored lessons update | Can require a fresh review without new checks |
| Ignored dependencies, external services, the full environment | Not covered |
| A temporary change restored before the final snapshot | Not detected |

Keep task-specific check scripts in `<task>/verify/`, which is bound
automatically. For another ignored input, list the exact repository-relative
file:

```yaml
run:
  id: lock
  argv: [bash, .mastermind/checks/lock.sh]
  cwd: .
  timeout_secs: 300
  inputs: [.mastermind/checks/lock.sh]
```

Input paths resolve from the repository root, whatever `cwd` is. A missing
declared file blocks verification. A pinned interpreter still doesn't freeze the
scripts it loads.

### Process supervision

| Event or platform | Behavior |
|---|---|
| macOS and Linux | The runner supervises the Unix process group |
| Completion, timeout or interruption | The group is terminated |
| Ctrl-C, `kill` or a closed terminal | The runner stops the group and records the receipt as `interrupted` |
| Detached child | Not contained by the group |
| SIGKILL of the runner | Cleanup can't run, and the receipt stays `pending` |
| Windows | Refused before the command starts |
| Cancellation | Checked right before the final atomic write. A later signal doesn't undo that write |
| Permissions | Normal file system and network access. There is no OS sandbox |

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
| Criterion ID | Unique, 1-64 ASCII bytes |
| Statement | Not empty, at most 2,048 bytes |
| Check references | 1-32 unique observed check IDs per criterion. All are required, and a shared ID reuses one observation |
| Rejected input | Nulls, control characters, placeholder statements and references to report-only checks |

| `acceptance status SPEC --json` result | Meaning |
|---|---|
| Check state | `current`, `missing`, `pending`, `failed`, `stale` or `unavailable` |
| Exit 0 | `requirements_satisfied` |
| Exit 1 | `blocked` or `not_declared` |
| Semantic fields | Always `semantic_accuracy: unknown` and `overall_task_completion: not_evaluated` |

A satisfied mapping means the mapped checks passed. Whether they test the
statement is the reviewer's call.

## Native executor

`run-task --exec` launches Claude Code with a task-bound prompt.

| Stage | Contract |
|---|---|
| Preflight | Repeated with the original baseline. Uses one iteration |
| Controller lock | Nonblocking, held from preflight through postflight. Another controller is refused, declared checks inside the executor are allowed |
| Context | Executor role, workflow and declared paths. Up to 12 literal title terms select up to 4 document hits |
| Before launch | Rechecks the selected code, project and document revisions, then the scoped profile sources and audience access |
| Consistency | Optimistic checks across separate stores. Revoking access can't take back bytes already delivered |
| Invocation binding | Repository, spec, baseline, iteration, exact context bytes, prompt digest, executable and version, permission policy |
| Delivery | Records `offered_to_process` and the byte counts on stdin. `model_use` stays `unknown`, because the receipt can't see what the model read |
| Profile audience | The project's configured profile client. `--profile-client ID` overrides it for one run and still needs an existing read grant |
| Claude executable | Resolved from absolute `PATH` entries and hashed as a stream, up to 2 GiB |
| Receipt | `invocation.json` is `pending` before launch, then records hashes, policy, byte counts and outcome. Raw prompts, profiles and process output are not stored |

| Default native policy | Contract |
|---|---|
| Requested tools | `Read,Edit,Write,Grep,Glob,Bash` |
| Permissions | `acceptEdits`, no permission prompts, no blanket Bash grant. Existing native rules decide command access |
| Session persistence and Chrome | Off |
| Adapter | Claude Code 2.1.267 or later within 2.1, with the required flags in local help |
| Success | Supported version and flags, matching initialization and one successful terminal result. Exit 0 alone is not enough |
| Tool errors | The run may repair intermediate errors. A final error or a permission denial fails the attempt |
| Mining | The process gets `MMCG_INPUT_ORIGIN=controller` and the receipt `input_origin: controller_generated`, so Mastermind capture skips the run |
| Inherited MCP, hooks, authentication, model and configuration | Used as configured, not verified |
| File scope | Audited after execution |
| Completion | Still needs the report, verification, acceptance and review gates |

| Limit | Value |
|---|---|
| Wall time | 1-7,200 seconds, default 1,800 |
| Turns | 1-100, default 40 |
| Input | 128 KiB |
| Process output, one protocol line | 16 MiB, 1 MiB |
| Version and help probe output | 64 KiB |
| Receipt | 128 KiB |

`mastermind init --exec-timeout N` and `--exec-max-turns N` set project
defaults in `.mastermind/setup.json`, and an explicit `run-task` flag wins.
Preparation and probes have their own bounded deadlines.

Native execution and native review need macOS or Linux. On Windows they refuse
before they change approval, iteration, review pins or execution requirements,
and `--exec --pre-only` still works for a manual preflight. On a supported
platform, a failed probe still invalidates the old approval before a new attempt
is prepared. A task that already requires native execution keeps that
requirement, so continue it on a supported platform. Don't delete task state to
reset its baseline or iteration budget.

[Lens](lens.md) shows the recorded delivery next to the current context preview,
see [the private Lens view](persona-context.md#private-lens-view).

### Guarded execution

```bash
mastermind run-task .mastermind/tasks/001-feature/spec.md \
  --exec --guarded-exec --auto-review
```

Guarded mode puts a private hook in front of each native tool call and allows
only what the approved task needs.

| Guarded contract | Rule |
|---|---|
| Platform and adapter | Unix, Claude Code 2.1.267 or later within 2.1, required flags in local help |
| Task | Explicit file scope and observed `verify[].run` checks |
| Edit, Write | Exact declared paths and the task's executor report. Controller and configuration paths and static symlink or hardlink aliases are rejected |
| Read, Grep, Glob | Path checks inside the repository |
| Bash | Only the exact `verification run` commands the controller generates |
| Other tools and subagents | Rejected |
| Native settings | `dontAsk`, restricted mode, isolated settings, private PreToolUse hook, strict empty MCP, slash commands off |
| Binding | Invocation, session, repository, task revision, intake, policy and expiry |
| Hook runner | Pins the Mastermind executable, at most 256 MiB, read with an identity check |
| Decision log | Tool name, input hash and decision. No raw tool arguments |
| Successful receipt | Every observed tool call matches a durable allow decision, no call was denied, artifacts are current |
| Failed or missing log | Unknown counts stay null and a successful result is blocked |
| Receipt schema | [v2](../../schemas/invocation-receipt-v2.schema.json). The default executor and reviewer write v1 |

If preparation records `invocation_guard_runner_too_large`, the Mastermind
binary is over 256 MiB. A release build fits:

```bash
cargo build --release --manifest-path mcp/servers/mmcg/Cargo.toml
```

Then run `mcp/servers/mmcg/target/release/mmcg`. Timeouts, interruption, a
changed identity and an unavailable runner each record their own reason, with
no raw file system errors.

| What guarded mode does not cover | Consequence |
|---|---|
| A native hook failure | The client can fall back to its own permissions, see [hook failure behavior](https://code.claude.com/docs/en/hooks#timeouts). Missing mediation blocks success but can't undo an effect |
| An allowed verification process | Runs with your normal file system and network access |
| Managed native policy | The requested isolation is recorded, the managed policy is not verified |
| A race after a path check | No OS containment and no atomic file-open guarantee |
| Effects outside native tool calls | Not observed |

Native [permission rules](https://code.claude.com/docs/en/permissions#read-only-commands)
still decide what a command may do.

## Native semantic reviewer

| Invocation | Effect |
|---|---|
| `review-task run` | Records one review of a pending held task |
| `run-task --auto-review` | Also evaluates completion under the controller lock |
| `--auto-review` without `--exec` | Keeps the iteration. Runs no preflight, executor or checks |
| `--exec --auto-review` | Reviews a held audit. With `--auto-follow-up`, at most one executor retry and one fresh review |
| Negative result | Stops, unless the [semantic follow-up](#one-semantic-follow-up) applies |
| Unknown, failed or unresolved-history result | Stops without a semantic retry |

| Reviewer input | Coverage |
|---|---|
| Evidence manifest and patch | Actual bytes, including tracked files hidden by Git index flags |
| Separate staged patch | Index-only and tracking changes |
| File modes | Included |
| Git reads | Repository fsmonitor and external diff hooks are off |
| Binary, inaccessible, unsupported or oversized input | Rejected, never silently left out |

| Native reviewer setting | Value |
|---|---|
| Adapter | Claude Code 2.1.290 or later within 2.1, with the required flags in local help |
| Tools | Read, Grep, Glob, plus StructuredOutput for the final answer. The adapter passes `--json-schema` with the closed reviewer answer schema |
| Permissions | `dontAsk`, no permission prompts, restricted mode |
| MCP | Strict empty configuration |
| Off | Slash commands, session persistence and Chrome |
| Initialization | Must match the policy. A tool error or denial fails the review. A StructuredOutput answer the schema rejects is not an error if the same run later succeeds. Tool calls need unique non-empty IDs |
| Personal profile | Not injected |
| Authentication and model | Native selection |
| Managed policy, reviewer identity and independence | Not verified |

| Limit | Value |
|---|---|
| Source inventory | 10,000 files, 4 MiB per file, 64 MiB per snapshot |
| Patch, complete prompt | 64 KiB, 128 KiB |
| Terminal result | 1 MiB |
| Wall time | 1-7,200 seconds, default 600 |
| Turns | 1-100, default 30 |
| Review invocation receipt | 256 KiB |

`mastermind init --review-timeout N` and `--review-max-turns N` set project
defaults in `.mastermind/setup.json`. An explicit `--timeout` or `--max-turns`
on `review-task run`, or `--review-timeout` or `--review-max-turns` on
`run-task`, wins.

| Review evidence step | Contract |
|---|---|
| Before preparation and probes | Revokes the earlier semantic approval and writes a pending `review-invocation.json`. The executor receipt stays |
| Assessment | Read only from the final native result string, parsed strictly. Extra fields, duplicate keys, code fences and other output files are rejected. A successful result must carry an equal `structured_output` |
| Failure codes | `invocation_native_structured_output_invalid`, and `invocation_native_max_turns` or `invocation_native_structured_output_retries` for a run that hits its turn or schema-retry limit |
| Reviewer metadata | The controller sets `reviewer.kind: llm` and the observed model name |
| Binding | Result and submission digests bind the semantic record to the exact receipt bytes |
| Missing or replaced native evidence | Blocks completion |
| `passed` receipt | A valid assessment exists. Its judgments can still block completion |
| Raw prompts and results | Not stored |

## Automatic repair

`--exec --auto-repair` retries the executor when a fresh check fails for an
ordinary reason inside the approved scope.

| Input | Requirement |
|---|---|
| Spec | Structured acceptance and an observed `run` for every verification entry |
| Fixed across attempts | Task lock, original baseline, approved spec, effective policy and profile selection |
| New each attempt | Preflight iteration, invocation ID and context packet |
| Receipts | Every check is a current success or a fresh normal nonzero exit, with at least one failure |
| Executor report | Names the failed command, has `partial` status and classifies defects as `implementation_defect` |
| Audit findings | Only the matching check or criterion failures, partial status or an unchanged expected file |
| Defect label alone | Not enough to retry |

| Loop event or boundary | Result |
|---|---|
| Missing or stale receipt, incomplete input, conflicting claim, scope or symbol drift, permission failure, timeout or technical failure | Stop |
| Feedback to the executor | Check and criterion IDs and the bound input and audit digests. No executor excerpts and no suggested fix |
| Freshness | Full verification inputs are checked again, `assume-unchanged` files included |
| Check executable | Pinned before the first native call and checked around later calls |
| Iteration budget | Cumulative, prior preflights included. 1-20, default 3. No unlimited budget and no `--force-iteration` |
| Invocation budget | Each attempt keeps its own time and turn limit |
| Stopped loop | Records `run_preflight`. Ordinary resume and `--post-only` can't skip it |
| Held audit | Ends repair and moves to semantic review |

A retry doesn't find the cause of a failure and doesn't prove the tests were
left intact. The review still checks scope, assertion quality and whether the
result is useful.

### One semantic follow-up

```bash
mastermind run-task .mastermind/tasks/001-feature/spec.md \
  --exec --auto-review --auto-follow-up --max-iterations 3
```

| Admission | Requirement |
|---|---|
| Mechanical state | Held audit, successful native executor, current checks |
| Semantic source | The current pinned native review, with unchanged supporting files |
| Criteria | At least one `unsatisfied` criterion with a reason and evidence, and no `unknown` criterion |
| Other judgments | Verification quality, scope control and proportionality are all `satisfied` |
| Project history | Both the context and the lessons decision are `no_change` |
| Contract | Same approved spec, scope, baseline, profile selection and executable pins |
| Budget | One semantic retry per controller invocation, inside the cumulative 1-20 iteration limit |
| Flags | Needs `--exec --auto-review`. Refuses `--pre-only`, `--post-only` and `--force-iteration` |

| Follow-up step | Behavior |
|---|---|
| Feedback | The cited unmet criteria and source digests, at most 32 KiB. Reviewer reasons are passed on as unverified data |
| Execution | A new preflight iteration and native invocation. No failed check is invented |
| Validation | Fresh checks, audit and a separate review before completion |
| Second negative review or changed evidence | Stop. There is no second semantic retry |
| With `--auto-repair` | Mechanical retries follow their own rules and share the same iteration budget |
| Knowledge or scope changes | Need an explicit follow-up outside this step |

The gates bind the feedback to its source, but they don't check the reviewer's
reading of it. The rules are in
[auto_repair.rs](../../mcp/servers/mmcg/src/auto_repair.rs) and
[run_task.rs](../../mcp/servers/mmcg/src/run_task.rs).

## Task records are parsed strictly

Mastermind writes these records itself, so it refuses any copy that a person or
another tool changed into a shape it never writes. A repeated JSON key or a
floating-point number makes the record invalid. Every SHA-256 digest, receipt
ID, invocation ID and review pin in them must be lowercase hex. Records that
4.0.x wrote still read.

| Record | Error when invalid |
|---|---|
| `state.json` | The parse error, and the task stops |
| Verification receipt `<task>/verification/<id>.json` | `receipt_invalid` |
| Executor invocation receipt | `invocation_receipt_invalid` |
| `review-invocation.json` | `review_invocation_receipt_invalid` |
| Preflight approval | `preflight_approval_invalid` |
| `review-task submit --report` file | `semantic_review_report_invalid` with the line and column |
| Guarded execution decision log | `invocation_guard_invalid_json` |

Output that a native client prints, such as stream events and the reviewer
result, still refuses repeated keys but may contain numbers with a fraction.

## Older task state

A task written by an earlier release keeps working where the table says so.
Otherwise finish it with the release that started it, or start a new task.

| State | Behavior |
|---|---|
| `state.json` without `format_version` | Read as format 1, the current format |
| `state.json` without `format_version` and an `executor-report.md` or `audit.md` beside a spec not named `spec.md` | Refused, because before 4.0 every spec in a folder shared these two names. The error lists one `mv` or `rm` command per old file. Run the ones for files of this task, move other files out of the folder, then run again. A spec that is itself named `audit.md` is not counted |
| `state.json` with a newer `format_version` | Refused, with a message to continue with the newer binary |
| Newer state, manifest or approval read by an older release | Refused. Finish or close open tasks before you downgrade |
| State without an initial-worktree manifest | Audited from commit to worktree, with no exemption for earlier local changes |
| Initial-worktree manifest with `schema_version` 1 | Readable, but preflight and postflight fail with `initial_worktree_schema_outdated`. Start a new task |
| Manifest without `max_total_bytes` | Read against 1 GiB |
| State without `snapshot_max_total_bytes` | Uses 32 MiB |
| Approval receipt with `schema_version` 1 | Valid for its own audit. Reapproval fails with `preflight_approval_schema_outdated`. Start a new task |
| State without retained approval files | `--pre-only` can keep earlier conditions only when the current spec matches the saved approved hash and the repository and baseline binding is valid. The receipt labels this `legacy_prior_approval`. Restore the approved spec first, run preflight, then revise and reapprove |
| Review receipt with schema v1 | Checked against its own policy, v1 (Claude Code 2.1.267 or later, Read, Grep, Glob) or v2. A runner upgrade doesn't reopen it. A new approval needs the current contract |
| `mode: lite` or `mode: standard` | Still accepted. New specs use `verified` or `strict` |
| Task without a structured review | Completes from a bound audit snapshot and `history-review.md` decisions marked `updated` or `not applicable`, with a reason |
