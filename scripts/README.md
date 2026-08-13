# Scripts

Repository validation, packaging, release, and smoke-test scripts. Run scripts
from the repository root unless a section says otherwise.

## Common entry points

| Goal | Command |
|---|---|
| Full deterministic gate | `just check` |
| Repository contracts only | `just validate` |
| Native npm tarball smoke | `just npm-smoke-native` |
| Index benchmark | `just benchmark-index` |

The `just` recipes are the canonical developer interface. Call an individual
script directly when diagnosing that script.

## `validate.py` — repository contract validator

Runs the deterministic repository-level checks that do not belong in the Rust,
npm, or model-backed test suites. CI executes it on every change.

### What it checks

- artifact frontmatter, names, versions, domains, links, and template mirrors;
- exact MCP tool count plus read/write annotations, and parity of the public
  declarative-fact schema with its CLI/MCP/Lens ingestion boundary;
- portable skill adapters and one behavioral eval case per shipped skill;
- planner/executor/auditor ownership and structured-report schema parity;
- GitHub Action SHA pins, required-check routing, Docker runtime packaging, and
  the audit publication security contract;
- npm package/version/platform shape, README badge alignment, and workflow-bundle staging parity;
- answer-leak clues in adversarial eval fixture source trees.

### Run directly

```bash
# One-time setup
python3 -m venv .venv
.venv/bin/pip install --require-hashes -r scripts/requirements.txt

# Run
.venv/bin/python scripts/validate.py
```

Exit code is `0` on clean and `1` when errors exist. Warnings do not fail the
run.

### Boundaries

- It does not compile Rust, execute npm, or call a model.
- It cannot prove that a prompt behaves correctly; `evals/runner.py` covers
  selected adversarial behaviors.
- It validates configured workflow structure and Docker packaging invariants;
  the CI image smoke supplies the hosted container proof.

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

## `configure-github-protections.sh` — live release controls

Prints the required `main`, `npm-v*`, and `npm-prod` settings by default. An
admin-authenticated maintainer can apply them explicitly:

```bash
scripts/configure-github-protections.sh
scripts/configure-github-protections.sh --apply
```

The script never reads or replaces environment secrets. It configures the npm
reviewer/tag boundary and ensures every unfiltered required workflow is present
in the active `main` ruleset. Self-review prevention stays disabled by default
so a single maintainer cannot deadlock a release. Enable it only with a distinct
eligible reviewer:

```bash
scripts/configure-github-protections.sh \
  --reviewer another-maintainer \
  --prevent-self-review \
  --apply
```

## Registry release smoke

The publish workflows run two post-publication checks against the public
registries, not the workspace build:

- `smoke-installed-npm-release.sh` installs the exact root npm package version
  into an isolated temporary project, verifies the selected native package and
  binary version, then exercises index, third-party adaptation, key generation,
  signed import/query, and a two-repository team map;
- `smoke-installed-crate-release.sh` installs the exact crate version into an
  isolated Cargo root and verifies the shipped binary plus the `facts`, `team`,
  and `review` command surfaces.

Both scripts retry registry propagation for a bounded period and fail the
release workflow if the public version cannot be installed or exercised. They
are release gates only; local validation never publishes a package.
