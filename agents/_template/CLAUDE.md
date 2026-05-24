---
name: <project-type-slug>
description: <CLAUDE.md template for <kind of project>. Use as a starting point for <when>.>
metadata:
  version: 0.1.0
  authors:
    - <github-handle>
  tags:
    - claude-md
    - <language-or-framework>
---

<!--
This is a CLAUDE.md template. Copy the body below into your project's CLAUDE.md
and fill in the <PLACEHOLDERS>. Delete this comment block and the frontmatter
when you copy — they belong to the template, not the project.
-->

# <PROJECT_NAME>

<One paragraph: what this project is, what it's not.>

## Quick orientation

- **Language/framework:** <e.g., Python 3.11 + FastAPI>
- **Entry point:** <e.g., `src/main.py`>
- **Where the interesting code lives:** <e.g., `src/services/`>
- **Where it doesn't:** <e.g., `vendor/`, `migrations/` — read-only>

## Commands

```bash
# Run locally
<COMMAND>

# Run tests
<TEST_COMMAND>

# Lint / typecheck
<LINT_COMMAND>
<TYPECHECK_COMMAND>

# Format
<FORMAT_COMMAND>
```

## Conventions

- <Convention 1 — e.g., "All new endpoints go under `src/api/v2/`">
- <Convention 2 — e.g., "Use SQLAlchemy 2.0 style (no legacy `query()`).">
- <Convention 3>

## Common pitfalls

- <Pitfall 1 — e.g., "Don't run migrations against prod without `--dry-run` first.">
- <Pitfall 2>

## When in doubt

- <Where to look — e.g., "See `docs/architecture.md` for the request flow.">
- <Who to ask — only if applicable; usually omit for OSS projects.>
