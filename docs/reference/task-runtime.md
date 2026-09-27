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
| Context selection | Executor role, workflow and declared paths |
| Invocation binding | Repository, spec, baseline, iteration, exact context bytes, prompt digest, executable/version and permission policy |
| Stdin delivery | Records `offered_to_process`, without proving model use |
| `--profile-client ID` | Selects an existing grant. Cannot grant access or change the profile |
| Receipt | `invocation.json` is pending before execution, then records hashes, policy, byte counts and outcomes |
| Excluded receipt data | Raw prompts, profiles and process output |

| Native policy or result | Contract |
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

## Native semantic reviewer

| Invocation | Effect |
|---|---|
| `review-task run` | Records one review of a pending held task |
| `run-task --auto-review` | Also evaluates completion under the controller lock |
| Without `--exec` | Keeps the iteration. Runs no preflight, executor or checks |
| With `--exec` | Can follow repair. Runs once after Held |
| Negative, unknown, failed or unresolved-history result | Stops. A judgment cannot trigger another repair attempt |

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
