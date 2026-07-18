# Scripts

Repo-level scripts, each documented at the top of its source.

## validate.py — artifact validator

Enforces [`docs/conventions.md`](../docs/conventions.md) against every artifact in the repo. Runs in CI (see [.github/workflows/validate.yml](../.github/workflows/validate.yml)) and locally.

### What it checks

- **Frontmatter** parses as YAML and is present in every artifact file
- **`name:` field** is kebab-case and matches the file/folder slug (§1.2)
- **`description:` field** is present and non-empty (warning if shorter than 40 chars)
- **`metadata.version`** is present and matches SemVer (§6)
- **`metadata.authors` / `metadata.tags`** are lists if present
- **First tag** matches the domain folder for skills/prompts (warning, see §2.3)
- **Domain folder** is in the conventions.md whitelist (§1.3) — for skills/ and prompts/
- **`[[slug]]` cross-references** resolve to an artifact's `name:` somewhere in the repo

### Run locally

```bash
# One-time setup
python3 -m venv .venv
.venv/bin/pip install -r scripts/requirements.txt

# Run
.venv/bin/python scripts/validate.py
```

Exit code is `0` on clean, `1` if any errors. Warnings don't fail the run.

### What it does NOT check

- That `description:` actually leads with a verb (heuristic, too noisy)
- That every relative `[link](path)` resolves (separate concern, big surface)
- Tag semantics beyond "is a list" and "first tag matches domain"
- Whether the artifact actually works — that's the author's job

### Excluded paths

- `_template/` directories — they show example syntax, not real references
- Top-level `docs/` — illustrates `[[slug]]` patterns as documentation
- `research/`, build artifacts (`target/`, `node_modules/`, `__pycache__/`)

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
