# Structured report schema

Every `mastermind-task-executor` and `mastermind-auditor` reply emits a
fenced-YAML "structured tail" alongside its markdown prose. The tail is wrapped
in HTML-comment sentinels so the planner can extract it deterministically with
a single regex.

The defect `kind:` vocabulary is the closed set defined in
[`defect-taxonomy.md`](defect-taxonomy.md). Subagents MUST pick a listed kind
or use `kind: unclassified` as the escape hatch.

## Executor tail

Emitted at the very end of the executor's reply, after the prose sections
(Phases completed / Verification results / Files modified / Stopped because /
What I did NOT do). Format:

````markdown
<!-- mastermind:report-begin -->
```yaml
schema_version: 1
spec: .mastermind/tasks/<NNN>-<name>/spec.md
status: complete | partial | failed
phases:
  - id: "1.1"
    status: done            # done | pending | stopped_here | skipped
  - id: "1.2"
    status: done
  - id: "2.4"
    status: stopped_here
files_modified:
  - mcp/servers/mmcg/src/store.rs
  - mcp/servers/mmcg/src/fingerprint.rs
claims:
  - kind: function_added
    symbol: change_impact
    file: mcp/servers/mmcg/src/queries.rs
    signature: "pub fn change_impact(...) -> Result<ChangeImpactResponse, ImpactError>"
  - kind: integration
    from: handle_tools_call
    from_file: mcp/servers/mmcg/src/mcp.rs
    to: change_impact
    to_file: mcp/servers/mmcg/src/queries.rs
    relation: calls
defects:
  - kind: envelope_drift
    phase: "2.4"
    details: |
      Test asserted on the raw `handle_tools_call` return, but the dispatcher
      wraps every payload in `{ "content": [{ "type": "text", "text": <json> }] }`.
      `cosmetic["class"]` is therefore not the field the assertion expects.
    remediation_hint: |
      Reuse `unwrap_content` from `mcp.rs::tests` (task 001). Replace
      `let cosmetic = read_env;` with `let cosmetic = unwrap_content(&read_env);`.
verifications:
  - cmd: "cd mcp/servers/mmcg && cargo test --locked --lib"
    result: pass
    observed:
      exit_code: 0
      tests_run: 298
  - cmd: "cd mcp/servers/mmcg && cargo test --locked --lib change_class"
    result: fail
    output_excerpt: "thread '...' panicked at ..."
```
<!-- mastermind:report-end -->
````

### Field meanings

- `schema_version`: always `1`. The normative machine-readable contract is
  `schemas/executor-report-v1.schema.json`. The Rust parser rejects unknown
  fields, malformed sentinels, unsupported versions, and file-backed reports
  larger than 1 MiB before YAML decoding instead of treating a mismatched
  report as empty.
- `spec`: absolute path to the spec file the executor is implementing.
- `status`:
  - `complete` — every phase landed, every Final-verification command exited 0
  - `partial` — at least one phase done, executor stopped before reaching Phase N
  - `failed` — Phase 1 couldn't even start (FIND mismatch on the first
    sub-step, environment broken, etc.)
- `phases[].status`:
  - `done` — phase's CHANGE TO content is in the file AND its VERIFY exited 0
  - `pending` — not yet attempted in this execution
  - `stopped_here` — the executor halted at this phase; populate the matching
    `defects[]` entry with details
  - `skipped` — planner explicitly dropped this phase mid-flight (e.g. Phase
    1.5 in task 002); list it for traceability
- `files_modified`: every path the executor's edits touched, relative to repo
  root. Must match `git diff --name-only HEAD` + untracked-new-files; this is
  the auditor's scope-creep anchor.
- `claims[]`: deterministic assertions about new symbols and new integration
  edges. Use `function_added` with an exact symbol/file/signature or
  `integration` with the changed caller and existing callee. Emit an empty
  array when the work makes neither claim. Do not encode subjective behavior.
- `defects[]`: zero or more defects. Empty array = clean run. Each entry MUST
  populate `kind` from the closed set in `defect-taxonomy.md` (or
  `unclassified`), `phase` of the failure, verbatim `details`, and a
  `remediation_hint` the planner can apply.
- `verifications[]`: every VERIFY command run, in execution order. Truncate
  `output_excerpt` to ~5 lines of the relevant error/diff. When the command
  exposes them, record the real exit code and tests-run count under `observed`;
  those values let the deterministic audit reject contradictory or vacuous
  pass claims.

## Auditor tail

Emitted at the very end of the auditor's reply. Format:

````markdown
<!-- mastermind:audit-begin -->
```yaml
spec: .mastermind/tasks/<NNN>-<name>/spec.md
verdict: held | drift | broken
files_in_scope: 7
files_in_diff: 7
scope_match: true
discrepancies:
  - kind: snapshot_caller_drift
    symbol: SessionStore
    spec_says: 45
    index_says: 38
    evidence: "git diff shows 7 callsites removed in src/api/*"
snapshot_drift:
  - symbol: commit_file
    pre_callers: 2
    post_callers: 2
    pre_signature: "pub fn commit_file(&mut self, pending: PendingFile) -> SqlResult<()>"
    post_signature: "pub fn commit_file(&mut self, pending: PendingFile) -> SqlResult<()>"
    delta: none
verifications_rerun:
  - cmd: "cd mcp/servers/mmcg && cargo test --locked --lib"
    result: pass
```
<!-- mastermind:audit-end -->
````

### Field meanings

- `verdict`:
  - `held` — every claim in the executor report survived independent
    verification; zero discrepancies
  - `drift` — partial drift; at least one discrepancy, none critical (warnings,
    minor scope creep, snapshot deltas with explanation)
  - `broken` — at least one critical discrepancy (scope creep without
    explanation, verify failed on re-run, signature drift that contradicts the
    spec's stated invariants)
- `discrepancies[]`: every finding that contributed to a non-`held` verdict.
  Each MUST use a `kind:` from the auditor section of `defect-taxonomy.md`.
- `snapshot_drift[]`: one entry per symbol in the spec's Pre-edit symbol
  snapshot, with pre/post caller counts and signatures and a `delta:` summary
  (`none` | `gained` | `lost` | `signature_changed`).

## Planner consumption

The planner (running `mastermind-task-planning` SKILL) extracts the tail with a
simple regex on the chat reply:

```text
<!-- mastermind:report-begin -->\n```yaml\n(?P<body>.*?)\n```\n<!-- mastermind:report-end -->
```

Then parses `body` as YAML. For each `defects[]` entry, the planner reads the
`kind:`, looks up the matching entry in `defect-taxonomy.md`, applies the named
fix template, and re-spawns the executor with the patched spec. This replaces
the manual prose-reading the planner did in tasks 001 and 002.

When `defects: []` and `status: complete`, the planner proceeds to spawn the
auditor.
