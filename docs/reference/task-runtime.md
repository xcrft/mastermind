# Task execution and receipt contracts

This reference covers local verification commands, native execution, semantic
review and automatic repair. For the user sequence, see [Workflow](../workflow.md).
All receipts below are owner-writable local records. They establish observed
execution and revision consistency, not independent attestation, test quality or
semantic correctness.

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
| Executable | Top-level executable bytes |
| Unsupported entries | Symlinks, submodules and conflicts rejected by bounded repository reads |

| Input | Limit |
|---|---|
| Observed files | 10,000 |
| Individual / total file bytes | 4 MiB / 64 MiB |
| Git response | 8 MiB |
| Executable | 128 MiB |
| Receipt JSON | 128 KiB |

| Change or limitation | Effect |
|---|---|
| Incomplete or changed bound input | Receipt withheld |
| Dependency edit, staging, commit, executable replacement or new preflight | Can invalidate the receipt |
| Receipt change during unfinished review | Invalidates that review |
| Final completion | Rechecks receipt bindings |
| Tracked context update | Can require new checks and audit |
| Ignored lessons update | Can require fresh review without changing verification inputs |
| Ignored dependencies, external services, full environment | Outside snapshot coverage |
| Temporary mutation restored before the final snapshot | Not detected by equal before/after snapshots |
| Pinned interpreter | Does not freeze scripts or establish assertion quality |

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
| Executor wall time / turns | 1–7,200 seconds / 1–100, defaults 1,800 / 40 |
| Input | 128 KiB |
| Process output / protocol line | 16 MiB / 1 MiB |
| Version/help probe output | 64 KiB |
| Executor receipt | 128 KiB |

Preparation and probes have separate bounded deadlines. Executor and reviewer
share Unix process-group supervision. Windows is unsupported.

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
| Decision log | Tool name, input hash and decision. No raw tool arguments |
| Successful receipt | Every observed tool call matches a durable allow decision, no denied calls, current artifacts |
| Failed or missing log | Unknown counts stay null, successful publication is blocked |
| Receipt schema | [v2](../../schemas/invocation-receipt-v2.schema.json). Default executor and reviewer retain schema v1 |

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
| Wall time / turns | 1–7,200 seconds / 1–100, defaults 600 / 20 |
| Review invocation receipt | 256 KiB |

| Native reviewer setting | Required value or limit |
|---|---|
| Adapter | Claude Code 2.1.267 or later within 2.1, with required flags in local help |
| Tools | Read/Grep/Glob |
| Permissions | `dontAsk`, no permission prompts, safe/restricted mode |
| MCP | Strict empty configuration |
| Disabled | Slash commands, session persistence and Chrome |
| Initialization | Must match policy. Tool errors or denials fail review |
| Mined profile | Not injected |
| Authentication and model | Native selection |
| Managed policy, reviewer identity and independence | Unverified |

| Review evidence transition | Contract |
|---|---|
| Before preparation/probes | Revoke prior semantic approval and write pending `review-invocation.json`. Preserve executor receipt |
| Assessment source | Final native result only. Reject extra fields, duplicate keys, code fences or alternative output files |
| Reviewer metadata | Controller supplies `reviewer.kind: llm` and observed model name |
| Binding | Result and normalized-submission digests bind the semantic record to exact receipt bytes |
| Missing or replaced native evidence | Blocks completion |
| `passed` receipt | Valid assessment produced. Its judgments may still be `blocked` |
| Raw prompts and results | Not stored |
| Historical completion | Validate pinned schema-v1 receipt against its own contract. Runner upgrade alone does not reopen it |
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
