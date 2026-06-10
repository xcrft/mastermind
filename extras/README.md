# Extras

Non-core artifacts that extend Mastermind but are not part of the default install.

These are not installed by `mastermind init`. Copy what you need manually into `~/.claude/`.

## Contents

### `subagents/`

| File | Description |
|---|---|
| `mastermind-release.md` | Drafts commit messages and PR descriptions after a verified audit. Read-only — produces a draft, planner executes the git commands after user approval. |

### `skills/`

| Skill | Description |
|---|---|
| `pr-review/` | Review a pull request for correctness, security, and design issues — staff-engineer style. |
| `flaky-finder/` | Identify flaky tests by running the suite repeatedly and bisecting failures. |
| `doc-stub-sync/` | Sync local doc stubs with online sources — diff by hash, refetch only what changed. Ships with a Python helper script. |

### `prompts/`

| File | Description |
|---|---|
| `api-shape-explorer.md` | Explore an API's shape interactively before writing an integration. |
