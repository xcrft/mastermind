# Task execution and receipt contracts

This reference covers local verification commands, native execution, semantic
review and automatic repair. For the user sequence, see [Workflow](../workflow.md).
All receipts below are owner-writable local records. They establish observed
execution and revision consistency, not independent attestation, test quality or
semantic correctness.

## Initial worktree ownership

The first successful preflight records the original Git commit and a separate
`state.json.initial_worktree` manifest of existing local changes. Their bounded
regular-file preimages are retained under `<task>/preflight-inputs/<sha256>`.
Keep these artifacts with the task when moving or recovering its checkout.

| Input or transition | Contract |
|---|---|
| Unchanged pre-existing tracked edits, deletions and untracked files | Excluded from the controller's task file set |
| New change to prior work, including restoration to commit contents | Included; normal scope and report checks apply |
| Symbol additions/removals in initially dirty source | Compared with its retained preflight bytes |
| Retry, reset, repair or reapproval | Retains the first manifest and original Git baseline |
| Missing or changed retained preimage | Blocks evidence; restore the exact retained artifact |
| Legacy state without a manifest | Keeps whole commit-to-worktree auditing; no retroactive exemption |
| Standalone audit-spec and CI | Preserve their explicit commit-to-worktree scope |
| Freshness and semantic review | Continue to bind current execution inputs; ownership grants no writes |

Initial dirty inputs are limited to 1,000 entries, 512 KiB of manifest metadata,
the indexer's per-file byte limit and 32 MiB of retained file bytes. Unsupported
or incomplete input fails preflight before handoff. This manifest establishes
local attribution, not authenticated authorship. Release draft statistics use
the original commit for the selected task files and separately list files
changed from the initial worktree.

## Dependency-cycle risk

Preflight checks cycle membership for normalized `touches`, `creates` and
`expected_docs` paths. Without declared file scope, it uses legacy file mentions.
Malformed frontmatter or invalid paths fail the risk check. Body references do
not expand an explicitly declared scope.

For each task file, the search follows indexed imports in both directions until
it finds a cycle witness or exhausts one direction. Paths may leave the task's
files and return. Unrelated components do not need to fit the global cycle-query
limit. An unindexed new file has no indexed import edges.

| Boundary | Contract |
|---|---|
| Shared search work | 50,000 neighbor visits, raw C++ includes and include resolution steps |
| Retained paths and C++ lookup strings | 50,001 unique strings and 8 MiB of UTF-8 bytes |
| Query budget | At most 10 seconds and 250,000 SQLite progress ticks, subject to a tighter caller budget |
| Incomplete search | Discards partial membership and fails preflight as unknown risk |
| SQL failure or cancellation | Fails preflight without approving an empty risk report |

Reverse C++ lookup visits matching candidates while scanning raw include records.
A cycle witness stops the scan and discards the partial lookup. Later task roots
may restart that scan within the shared work budget. A completed lookup is reused
for subsequent reverse steps, selecting possible exact and suffix basenames before
applying the same exact-before-suffix resolver as forward traversal. Cached steps
copy only the candidate being visited. All lookups share one SQLite read snapshot.
The lookup is discarded with the search.

The result describes syntactic indexed imports. It does not establish runtime
cycles or source freshness. C++ suffix resolution can include multiple candidate
headers when include roots are unavailable. The global dependency-cycle query
retains its separate 50,000-pair whole-graph limit.

If the task search is incomplete, inspect the declared paths and their indexed
dependencies before retrying. Keep the complete task scope. Repair an unreadable
or stale index through the normal indexing flow, then rerun preflight.

## Revise an approved task

After changing an approved spec, explicitly approve the revised contract:

```bash
mastermind run-task .mastermind/tasks/001-feature/spec.md --pre-only
```

Each successful preflight retains the exact spec and a validation receipt under
`<task>/preflight-approvals/`, pinned by `state.json.preflight_approval_sha256`.
Keep the retained spec, receipts and initial-worktree preimages with the task.

| Reapproval input or transition | Contract |
|---|---|
| Unchanged FIND and pre-edit symbol conditions in an amended spec | Retain their prior successful validation; implemented replacements need not restore old text or symbols |
| Re-run preflight with the same retained spec | Rechecks current preconditions in full; invalid or unreadable targets still block approval |
| New or changed literal, symbol, signature, caller count, file or language scope | Requires current verification |
| Current declarations, creates, verification, acceptance and strict requirements | Checked in full against the revised contract |
| Original Git baseline and initial-worktree manifest | Retained; implementation stays in the task delta |
| Approval attempt | Consumes the cumulative iteration budget, including failures |
| Native invocation requirement | Retained; a receipt from an older spec or iteration cannot satisfy the new attempt |
| Ordinary resume, post-only or restoring old spec bytes | Does not bypass required explicit preflight |
| Missing or changed retained approval ancestor | Blocks reuse; restore the exact artifact |

Historical conditions are prior local validation, not fresh caller measurements.
The current risk report covers newly evaluated snapshot claims. Approval chains
are limited to 128 records and 32 MiB of retained receipt/spec reads per check.

For a legacy state without retained approval artifacts, explicit `--pre-only`
can retain historical conditions only when the current spec exactly matches its
saved approved hash and the repository/baseline binding is valid. The new
receipt labels this basis `legacy_prior_approval`; it does not reconstruct an
original attempt or native execution. When recovering such a task, restore the
approved spec first, run explicit preflight, then revise it and reapprove. Changed
conditions without that prior contract must pass current checks.

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
| `argv` | Executed directly, with null stdin and inherited environment |
| Shell syntax | Requires an explicitly declared shell executable |
| `cmd` | Must equal displayed argv. Simple arguments are unquoted, others use single quotes with embedded apostrophes written as `'\''` |
| Display mismatch | Preflight reports the required `cmd` |

| Declaration | Limit |
|---|---|
| Runs per spec | 32 |
| Unique run ID | 1–64 ASCII letters, digits, `_` or `-` |
| Arguments | 64 maximum, 8 KiB each, 32 KiB combined, no NUL bytes |
| `cwd` | `.` or a canonical repository-relative directory |
| Timeout | 1–3,600 seconds |
| Captured stream | 1 MiB each for stdout and stderr |

| Receipt or transition | Contract |
|---|---|
| Canonical location | `<task>/verification/<id>.json` |
| Flat legacy location | `<spec-name>.verification/` beside the run-state file |
| Recorded inputs | argv, cwd, resolved executable hash and repository/spec/baseline/iteration bindings |
| Recorded result | Observed exit, duration, stream byte counts and hashes |
| Excluded data | Raw output and environment values |
| New attempt | Replaces the previous receipt with `pending` before execution |
| Concurrent runner for the same task | Refused |
| Failure, interruption, timeout, output limit or unfinished attempt | Cannot reuse an older success |
| Postflight | Requires the latest successful current receipt for every observed declaration and the normal executor report |
| Declaration without `run` | Keeps the weaker reported-result contract |

Running a check neither requires nor modifies the executor report.

### Snapshot coverage and freshness

| Snapshot component | Coverage |
|---|---|
| Task and Git state | Spec, HEAD, index and baseline |
| Repository files | Tracked and nonignored untracked files outside `.mastermind/` |
| Verification kit | Regular files under the current spec's sibling `verify/` directory, including ignored files |
| Additional check inputs | Explicit `verify[].run.inputs` repository-relative files, including ignored files |
| Executable | Top-level executable bytes |
| Tracked symlinks on Unix | Exact relative link text is bound without following the link. Targets must remain lexically inside the repository and exist through no-follow paths. Absolute, escaping, dangling and chained targets are rejected |
| Link target contents | Bound independently only when tracked, nonignored or an explicit verification input. Ignored target bytes are not read through the link |
| Dirty or untracked symlinks | Initial-worktree and source readers still reject them before handoff; tracked link support does not grant source or write access through aliases |
| Unsupported entries | Submodules and conflicts rejected before approval for observed checks |

| Input | Limit |
|---|---|
| Observed files | 100,000 |
| Individual / total snapshot file bytes per pass | 128 MiB / 512 MiB |
| Retained snapshot file content | One 64 KiB streaming buffer |
| Verification spec reads | 4 MiB |
| Git response | 8 MiB |
| Executable | 128 MiB |
| Receipt JSON | 128 KiB |
| Verification kit entries | 1,024 files and directories per task |
| Additional inputs | 128 unique files per check |

Each snapshot compares two complete passes. Preflight reads at most 1 GiB of
file content, and command verification takes snapshots before and after execution
for at most 2 GiB. Each regular file also receives a one-byte EOF probe per pass.
All reads share the operation deadline and cancellation signal. Growth, shrinkage,
replacement and unreadable inputs fail the snapshot. File content streams into
the unchanged version-1 digest, so previously admitted inputs keep their digest.
The caps bound I/O separately from the streaming buffer and do not change parser
or configuration limits. The separate working-tree diff reader keeps its 5 MiB
per-file cap. Changing a larger file can still make `audit-spec` fail closed with
`index_stale`. Larger snapshot coverage does not waive the diff requirement.

| Change or limitation | Effect |
|---|---|
| Unsupported, over-limit or unreadable snapshot | Preflight rejects observed checks early; direct runner records `unavailable` with a specific reason. Before spawn there is no command exit; after spawn the observed exit is retained |
| Changed bound input | `input_changed`; receipt cannot satisfy checks |
| Dependency edit, staging, commit, executable replacement or new preflight | Can invalidate the receipt |
| Receipt change during unfinished review | Invalidates that review |
| Final completion | Rechecks receipt bindings |
| Tracked context update | Can require new checks and audit |
| Ignored lessons update | Can require fresh review without changing verification inputs |
| Ignored dependencies, external services, full environment | Outside snapshot coverage |
| Temporary mutation restored before the final snapshot | Not detected by equal before/after snapshots |
| Pinned interpreter | Does not freeze scripts or establish assertion quality |

Keep task-specific check scripts in `<task>/verify/`. This directory is bound
automatically; receipt and controller files elsewhere in the task remain mutable.
For other ignored check inputs, declare exact repository-relative regular files:

```yaml
run:
  id: lock
  argv: [bash, .mastermind/checks/lock.sh]
  cwd: .
  timeout_secs: 300
  inputs: [.mastermind/checks/lock.sh]
```

Input paths resolve from the repository root, independently of `cwd`. Changes to
the verification kit or declared inputs require fresh checks. Missing declared
files block verification. This binds named bytes, not arbitrary interpreter
dependencies, external services, or the semantic quality of assertions.

### Process supervision

| Event or platform | Behavior |
|---|---|
| macOS/Linux | Supervise the Unix process group |
| Completion, timeout or cooperative interruption | Terminate the group |
| Detached child | Not contained by the group |
| SIGKILL | Cleanup cannot run. Receipt stays pending |
| Windows | Execution rejected before spawn |
| Cancellation | Checked immediately before the atomic final write. A later signal does not revoke that publication decision |
| Permissions | Normal filesystem and network access. No OS sandbox |

## Acceptance mapping

```yaml
acceptance:
  - id: recovery
    statement: An expired token is rejected without changing the account.
    checks: [unit, integration]
```

| Declaration | Requirement |
|---|---|
| Criteria | 1–64 |
| Criterion ID | Unique, 1–64 ASCII bytes |
| Statement | Nonempty, at most 2,048 bytes |
| Check references | 1–32 unique known observed IDs per criterion. All required, shared IDs reuse one observation |
| Rejected input | Nulls, control characters, placeholder statements or report-only command references |

| Read-only `acceptance status SPEC --json` result | Meaning |
|---|---|
| Check state | `current`, `missing`, `pending`, `failed`, `stale` or `unavailable` |
| Exit 0 | `requirements_satisfied` |
| Exit 1 | `blocked` or `not_declared` |
| Semantic result | Always `semantic_accuracy: unknown` and `overall_task_completion: not_evaluated`. A satisfied mapping does not prove its statements |

## Native executor

| `run-task --exec` stage | Contract |
|---|---|
| Preflight | Repeated with the original baseline. Consumes one iteration |
| Controller lock | Nonblocking, covers preflight through postflight. Rejects another controller but allows declared checks inside the executor |
| Context selection | Executor role, workflow and declared paths. Up to 12 literal title terms select up to 4 document hits |
| Delivery check | Rechecks selected code/project/document revisions, then scoped profile sources and audience access before native launch |
| Consistency | Optimistic checks across independent stores. Revocation cannot retract bytes already delivered |
| Invocation binding | Repository, spec, baseline, iteration, exact context bytes, prompt digest, executable/version and permission policy |
| Stdin delivery | Records `offered_to_process` and offered byte counts. `model_use` remains `unknown` |
| `--profile-client ID` | Selects an existing grant. Cannot grant access or change the profile |
| Receipt | `invocation.json` is pending before execution, then records hashes, policy, byte counts and outcomes |
| Excluded receipt data | Raw prompts, profiles and process output |

| Default native policy or result | Contract |
|---|---|
| Requested tools | `Read,Edit,Write,Grep,Glob,Bash` |
| Permissions | `acceptEdits`, no permission prompts and no blanket Bash grant. Existing native rules decide command access |
| Session persistence and Chrome | Disabled |
| Success | Supported version/help flags, matching initialization and one successful terminal result. Exit 0 alone is insufficient |
| Tool errors | Intermediate errors may be repaired within the run. Final errors and permission denials fail the attempt |
| Miner origin | Process gets `MMCG_INPUT_ORIGIN=controller`, receipt gets `input_origin: controller_generated`. Mastermind capture skips it before writing state |
| Other inherited hooks | Unverified |

| Native transport | Limit |
|---|---|
| Executor wall time / turns | 1–7,200 seconds / 1–100, defaults 1,800 / 40. `mastermind init --exec-timeout N` and `--exec-max-turns N` set the project defaults in `.mastermind/setup.json`. An explicit `--exec-timeout` or `--exec-max-turns` wins. The defaults are never written |
| Input | 128 KiB |
| Process output / protocol line | 16 MiB / 1 MiB |
| Version/help probe output | 64 KiB |
| Executor receipt | 128 KiB |

Preparation and probes have separate bounded deadlines. Executor and reviewer
share Unix process-group supervision. Windows is unsupported.

On Windows, native execution and native review refuse before changing task
approval, iteration, review pins or execution requirements. Manual preflight
with `--exec --pre-only` remains available. A failed native probe on a supported
platform still invalidates the old approval before preparing a new attempt.

Tasks already recorded as requiring native execution keep that requirement.
Continue them on a supported platform or restore a known valid task backup.
Do not delete task state to reset its baseline or iteration budget.

| Boundary | Limit |
|---|---|
| Inherited MCP, hooks, authentication, model selection and configuration | Not independently verified |
| File scope | Checked after execution |
| Completion | Still requires the ordinary report, verification, acceptance and review gates |

Lens shows delivery metadata from the recorded invocation alongside the current
context preview. Matching revisions compare declared bytes, not model attention,
task acceptance or current checkout correctness. The preview itself records no
delivery. See [Profiles](mmcg.md#private-profiles-and-context-preview).

### Guarded execution

```bash
mastermind run-task .mastermind/tasks/001-feature/spec.md \
  --exec --guarded-exec --auto-review
```

| Guarded contract | Rule |
|---|---|
| Platform and adapter | Unix, supported Claude Code 2.1.267+ within 2.1, required flags in local help |
| Task | Explicit file scope, observed `verify[].run` declarations |
| Edit / Write | Exact declared paths and the task's executor report. Protected controller/configuration paths and static symlink/hardlink aliases are rejected |
| Read / Grep / Glob | Repository-scoped path checks |
| Bash | Exact controller-generated `verification run` commands only |
| Other tools and subagents | Rejected |
| Native settings | `dontAsk`, restricted mode, isolated settings, private PreToolUse hook, strict empty MCP, disabled slash commands |
| Binding | Invocation, session, repository, task revision, intake, policy and expiry |
| Hook runner | Pins the Mastermind executable; at most 256 MiB, with a bounded identity-checked read |
| Decision log | Tool name, input hash and decision. No raw tool arguments |
| Successful receipt | Every observed tool call matches a durable allow decision, no denied calls, current artifacts |
| Failed or missing log | Unknown counts stay null, successful publication is blocked |
| Receipt schema | [v2](../../schemas/invocation-receipt-v2.schema.json). Default executor and reviewer retain schema v1 |

If preparation records `invocation_guard_runner_too_large`, build with
`cargo build --release --manifest-path mcp/servers/mmcg/Cargo.toml` and invoke
`mcp/servers/mmcg/target/release/mmcg`. The repository's dev/test profile retains
limited debug information (`debug = 1`) to keep normal debug builds within this bound.
Runner timeouts, interruption, changed identity and unavailability have separate
fixed reasons; receipts contain no raw filesystem errors.

| Enforcement boundary | Meaning |
|---|---|
| Native command-hook failure | The client can fall back to native permissions. Missing observed mediation blocks success but cannot undo an effect |
| Allowed verification process | Runs with the user's normal filesystem/network access |
| Managed native policy | Requested isolation is recorded, managed policy is unverified |
| Race after a path check | No OS-level containment or atomic file-open guarantee |
| Coverage | Observed native tool calls, not every process or OS effect |

The adapter follows the native [hook failure behavior](https://code.claude.com/docs/en/hooks#timeouts)
and [permission rules](https://code.claude.com/docs/en/permissions#read-only-commands).
Lens reports mediation coverage and the recorded invocation separately from the
current context preview. Reconciliation does not establish model attention or
semantic correctness.

## Native semantic reviewer

| Invocation | Effect |
|---|---|
| `review-task run` | Records one review of a pending held task |
| `run-task --auto-review` | Also evaluates completion under the controller lock |
| Without `--exec` | Keeps the iteration. Runs no preflight, executor or checks |
| With `--exec` | Reviews a Held audit. A qualifying `--auto-follow-up` adds at most one executor retry and one fresh review |
| Negative result | Stops by default. Only the opt-in [semantic follow-up](#one-semantic-follow-up) can retry |
| Unknown, failed or unresolved-history result | Stops without an automatic semantic retry |

| Reviewer input | Coverage |
|---|---|
| Evidence manifest and patch | Actual bytes, including tracked files hidden by Git index flags |
| Separate staged patch | Index-only and tracking changes |
| Modes | Included |
| Reads | Repository fsmonitor and external diff hooks disabled |
| Binary, inaccessible, unsupported or oversized input | Rejected, not silently omitted |

| Reviewer input / transport | Limit |
|---|---|
| Source inventory | 10,000 files, 4 MiB/file, 64 MiB per snapshot |
| Patch / complete prompt | 64 KiB / 128 KiB |
| Terminal result | 1 MiB |
| Wall time / turns | 1–7,200 seconds / 1–100, defaults 600 / 30. `mastermind init --review-max-turns N` (1–100) and `--review-timeout N` (1–7,200) set the project default turns and seconds in `.mastermind/setup.json`. An explicit `--max-turns`, `--review-max-turns`, `--timeout` or `--review-timeout` wins. The defaults 30 and 600 are never written |
| Review invocation receipt | 256 KiB |

| Native reviewer setting | Required value or limit |
|---|---|
| Adapter | Claude Code 2.1.290 or later within 2.1, with required flags in local help |
| Tools | Read/Grep/Glob, plus StructuredOutput for the final answer. The adapter passes `--json-schema` with the closed reviewer answer schema |
| Permissions | `dontAsk`, no permission prompts, safe/restricted mode |
| MCP | Strict empty configuration |
| Disabled | Slash commands, session persistence and Chrome |
| Initialization | Must match policy. Tool errors or denials fail review. A StructuredOutput attempt rejected by the schema is not a tool error when the same run later succeeds. Reviewer tool calls need unique non-empty ids |
| Mined profile | Not injected |
| Authentication and model | Native selection |
| Managed policy, reviewer identity and independence | Unverified |

| Review evidence transition | Contract |
|---|---|
| Before preparation/probes | Revoke prior semantic approval and write pending `review-invocation.json`. Preserve executor receipt |
| Assessment source | Final native result string only, parsed strictly. Reject extra fields, duplicate keys, code fences or alternative output files. A successful result must also carry a `structured_output` equal to it. Failures record `invocation_native_structured_output_invalid`. A run that stops at its turn or schema-retry limit records `invocation_native_max_turns` or `invocation_native_structured_output_retries` |
| Reviewer metadata | Controller supplies `reviewer.kind: llm` and observed model name |
| Binding | Result and normalized-submission digests bind the semantic record to exact receipt bytes |
| Missing or replaced native evidence | Blocks completion |
| `passed` receipt | Valid assessment produced. Its judgments may still be `blocked` |
| Raw prompts and results | Not stored |
| Historical completion | Validate pinned schema-v1 receipt against its own contract: review policy v1 (Claude Code 2.1.267+, Read/Grep/Glob) or v2. Runner upgrade alone does not reopen it |
| New approval | Must satisfy the current contract |

## Automatic repair

| `--exec --auto-repair` input | Requirement |
|---|---|
| Spec | Structured acceptance and observed `run` for every verification entry |
| Fixed across attempts | Task lock, original baseline, approved spec, effective policy and profile selection |
| Fresh each attempt | Preflight iteration, invocation ID and context packet |
| Receipts | Every check is a current success or fresh normal nonzero exit, with at least one failure |
| Canonical report | Names the failed command, has `partial` status and classifies defects as `implementation_defect` |
| Audit findings | Only corresponding verification/criterion failures, partial status or an unchanged expected file |
| Defect label alone | Insufficient to authorize retry |

| Loop event or boundary | Result |
|---|---|
| Missing/stale receipt, incomplete input, conflicting claim, scope/symbol drift, permission failure, timeout or technical failure | Stop |
| Feedback | Check/criterion IDs and bound input/audit digests from the in-memory audit. No executor excerpts or suggested remediation |
| Freshness | Recheck full verification inputs, including `assume-unchanged` files |
| Check executable | Pin the top-level executable before the first native call and around later calls |
| Iteration budget | Cumulative, including prior preflights. 1–20, default 3. No unlimited budget or `--force-iteration` |
| Invocation budget | Each attempt retains its time/turn limit |
| Stopped loop | Records `run_preflight`. Ordinary resume and post-only cannot bypass it |
| Held audit | Ends repair and proceeds to semantic review |

Retry eligibility does not identify a failure's cause or prove tests were not
weakened. Review still checks scope, assertion quality and useful completion.

### One semantic follow-up

```bash
mastermind run-task .mastermind/tasks/001-feature/spec.md \
  --exec --auto-review --auto-follow-up --max-iterations 3
```

| Admission | Requirement |
|---|---|
| Mechanical state | Held audit, successful native executor and current checks |
| Semantic source | Current pinned native review and unchanged supporting files |
| Criteria | At least one `unsatisfied` criterion with a reason and evidence. No `unknown` criterion |
| Other judgments | Verification quality, scope control and proportionality all `satisfied` |
| Project history | Both context and lessons decisions are `no_change` |
| Contract | Same approved spec, scope, baseline, profile selection and executable pins |
| Budget | At most one semantic retry in this controller invocation, within the cumulative 1–20 iteration limit |
| Flags | Requires `--exec --auto-review`. Conflicts with `--pre-only`, `--post-only` and `--force-iteration` |

| Follow-up step | Behavior |
|---|---|
| Feedback | Cited unmet criteria and source digests, capped at 32 KiB. Reviewer reasons remain unverified data |
| Execution | New preflight iteration and native invocation. No invented failed-check result |
| Validation | Fresh checks, audit and a separate review before completion |
| Second negative review or changed evidence | Stop. No second semantic retry |
| Combined `--auto-repair` | Mechanical retries remain governed by their own admission rules and the same iteration budget |
| Knowledge or scope changes | Require explicit follow-up outside this automatic semantic step |

Limits and eligibility are defined in [auto_repair.rs](../../mcp/servers/mmcg/src/auto_repair.rs)
and [run_task.rs](../../mcp/servers/mmcg/src/run_task.rs). The deterministic gates
bind the feedback to its source. They do not verify the reviewer's interpretation.
