# Scripts

Repo-level scripts, each documented at the top of its source.

## `validate.py` — repository contract validator

Runs the deterministic repository-level checks that do not belong in the Rust,
npm, or model-backed test suites. CI executes it on every change.

### What it checks

- artifact frontmatter, names, versions, domains, links, and template mirrors;
- exact MCP tool count plus read/write annotations;
- portable skill adapters and one behavioral eval case per shipped skill;
- planner/executor/auditor ownership and structured-report schema parity;
- GitHub Action SHA pins and the audit publication security contract;
- npm package/version/platform shape and workflow-bundle staging parity;
- answer-leak clues in adversarial eval fixture source trees.

### Run locally

```bash
# One-time setup
python3 -m venv .venv
.venv/bin/pip install -r scripts/requirements.txt

# Run
.venv/bin/python scripts/validate.py
```

Exit code is `0` on clean, `1` if any errors. Warnings don't fail the run.

### Boundaries

- It does not compile Rust, execute npm, or call a model.
- It cannot prove that a prompt behaves correctly; `evals/runner.py` covers
  selected adversarial behaviors.
- It validates configured workflow structure, not GitHub-hosted runtime state.

### Excluded paths

- `_template/` directories — they show example syntax, not real references
- Build artifacts and local state (`target/`, `node_modules/`, `.mastermind/`,
  virtual environments, and caches).
- Templates are excluded from artifact discovery where they intentionally show
  placeholder syntax.

### Adding a new check

Edit `scripts/validate.py`. The validator collects `Issue` objects with `level: "error" | "warning"` and a message. The pattern is:

```python
def validate_artifact(a: Artifact) -> list[Issue]:
    issues = []
    # ... add your check ...
    if some_problem:
        issues.append(Issue(a.path, "error", "what's wrong"))
    return issues
```

When you add a check that flags many existing artifacts, **fix them in the same PR** so CI stays green.
