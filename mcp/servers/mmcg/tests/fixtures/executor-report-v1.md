## Task 001 — execution report

The prose is intentionally ignored by the deterministic parser.

<!-- mastermind:report-begin -->
```yaml
schema_version: 1
spec: .mastermind/tasks/001-example/spec.md
status: complete
phases:
  - id: "1.1"
    status: done
files_modified:
  - src/lib.rs
claims:
  - kind: function_added
    symbol: cancel_order
    file: src/lib.rs
    signature: "pub fn cancel_order(id: &str) -> Result<(), Error>"
  - kind: integration
    from: handle_cancel
    from_file: src/api.rs
    to: cancel_order
    to_file: src/lib.rs
    relation: calls
defects: []
verifications:
  - cmd: cargo test --locked
    result: pass
    observed:
      exit_code: 0
      tests_run: 12
```
<!-- mastermind:report-end -->
