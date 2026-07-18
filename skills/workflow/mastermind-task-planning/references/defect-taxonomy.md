# Recommended defect labels

`defects[].kind` is an open string in executor-report schema v1. These stable,
product-level labels make reports easier to group, but the controller does not
auto-repair a task from them. Use `unclassified` when none fits; preserve the
concrete evidence in `details`.

## Executor labels

### `contract_drift`

The spec contradicts the repository, an exact FIND block no longer matches, or
Goals/Scope/Acceptance Criteria disagree. Return to the planner; do not silently
reinterpret the contract.

### `missing_prerequisite`

An authorized file, dependency, service, credential, fixture, or other required
input is unavailable. Name the missing input and the check that established it.

### `environment_blocked`

The implementation cannot be verified because the runner, toolchain, database,
network dependency, or index is unavailable or materially stale. Separate this
from a failing implementation.

### `implementation_defect`

An in-scope implementation or focused test still fails after the bounded repair
loop. Include the final command, exit status, and short failure evidence.

### `verification_failed`

A required Final Verification command failed. Do not weaken the command or
acceptance criterion to convert the report to complete.

### `scope_mismatch`

The required solution would modify behavior or files outside approved Scope, or
the actual diff contains an unexplained path. Return to planning/user review.

### `report_malformed`

Required execution evidence could not be represented or validated under schema
v1. This is normally reported by the controller rather than by the malformed
report itself.

### `unclassified`

A concrete defect that does not fit the labels above. Explain it fully; do not
invent a more precise label without evidence.

## Auditor labels

Independent auditors may reuse the labels above and the deterministic audit
finding names such as `unexpected_file`, `missing_expected_file`,
`snapshot_caller_drift`, `snapshot_signature_drift`, `claimed_symbol_missing`,
or `missing_call_edge`. These are evidence categories, not workflow commands.

Historical task-specific failures belong in `.mastermind/tasks/_lessons.md` or
project documentation, not in this portable vocabulary.
