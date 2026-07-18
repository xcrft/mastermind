# Structured report schema v1

This reference explains the file-backed report consumed by the current
controller. The canonical JSON Schema is
`schemas/executor-report-v1.schema.json`; the Rust parser adds semantic
consistency checks.

## Transport

- File: `<task>/executor-report.md`
- Required sentinels: `<!-- mastermind:report-begin -->` and
  `<!-- mastermind:report-end -->`
- Encoding: UTF-8
- Maximum size: 1 MiB before YAML decoding
- Lifecycle consumer: `mastermind run-task <spec> --post-only`

The controller does not scrape chat, regex-route planner messages, patch specs,
or spawn agents from defect labels.

## Canonical tail

````markdown
<!-- mastermind:report-begin -->
```yaml
schema_version: 1
spec: .mastermind/tasks/001-example/spec.md
status: complete
phases:
  - id: plan-1
    status: done
files_modified:
  - src/example.rs
claims: []
defects: []
verifications:
  - cmd: "cargo test --locked --lib"
    result: pass
    observed:
      exit_code: 0
      tests_run: 12
```
<!-- mastermind:report-end -->
````

## Fields

- `schema_version`: exactly `1`.
- `spec`: non-empty path naming the contract.
- `status`: `complete`, `partial`, or `failed`.
- `phases`: schema-v1 execution-step evidence. `id` may be `plan-1` or a legacy
  phase ID; IDs must be unique. Status is `done`, `pending`, `stopped_here`, or
  `skipped`.
- `files_modified`: paths the executor says it changed. The controller derives
  the authoritative changed-file set from git.
- `claims`: optional deterministic assertions. Supported kinds:
  - `function_added`: `symbol`, optional `file` and `signature`.
  - `integration`: `from`, `to`, optional files and relation.
- `defects`: concrete blockers with `kind`, `phase`, `details`, and
  `remediation_hint`. Kinds are recommended labels, not an enforced enum.
- `verifications`: commands actually run, their `pass`/`fail` result, optional
  short output, and optional observed exit/test counts.

## Consistency rules

- `complete` requires no defects, all steps `done`, and no failed verification.
- `partial` and `failed` require at least one defect.
- Empty step IDs, duplicate step IDs, empty paths, empty defect fields, and
  empty verification commands are invalid.
- Unknown top-level or nested fields are invalid.

The parser projects supported claims and verification observations into the
deterministic audit. Status, steps, file evidence, and defects are validated for
consistency even when the mechanical audit independently derives repository
facts.

## Strict auditor advisory tail

````markdown
<!-- mastermind:audit-begin -->
```yaml
spec: .mastermind/tasks/001-example/spec.md
verdict: held | drift | broken
scope_match: true
discrepancies:
  - kind: scope_mismatch
    evidence: "<diff/index evidence>"
verifications_rerun:
  - cmd: "cargo test --locked --lib"
    result: pass
```
<!-- mastermind:audit-end -->
````

This output supports planner semantic review in Strict mode. It is not consumed
or persisted by the Rust controller today. Controller-owned `audit.md` and
`state.json` remain the machine lifecycle record.
