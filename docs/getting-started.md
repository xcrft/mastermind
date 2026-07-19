# Getting started

Mastermind has two independent layers:

1. A global CLI, workflow bundle, and MCP registration.
2. A local codegraph for each repository you choose to index.

You can install and connect Mastermind without initializing a project. `mastermind init` is only the convenience command for enabling the complete repository workflow.

## Requirements

- Node.js 24+ for the npm package
- Claude Code, Codex, Cursor, Continue, or another MCP stdio client
- Git for baseline-aware change and audit commands

Rust is not needed for npm installation. Source builds require Rust 1.96+.

## Install the CLI

Global installation is simplest when you use Mastermind across repositories:

```bash
npm install -g @xcraftmind/mastermind
mastermind --version
```

For a version pinned to one repository:

```bash
npm install -D @xcraftmind/mastermind
npx mastermind --version
```

## Connect an AI client

`mastermind setup` previews a redacted plan. Add `--write` to apply it.

| Client | User-scope command | Notes |
|---|---|---|
| Claude Code | `mastermind install` | Installs skills, subagents, and user-scope MCP |
| Codex | `mastermind install --client codex` | Installs skills and user-scope MCP |
| Cursor | `mastermind setup cursor --scope user --write` | User and project scopes supported |
| Continue | `mastermind setup continue --scope user --write` | Owns one `mastermind.yaml` file |
| Generic | See [generic MCP setup](integrations/generic-mcp.md) | Requires an explicit config path |

Client guides document removal, project scope, backups, and the exact config shape:

- [Claude Code](integrations/claude-code.md)
- [Codex](integrations/codex.md)
- [Cursor](integrations/cursor.md)
- [Continue](integrations/continue.md)
- [Generic MCP](integrations/generic-mcp.md)

No repository and no `mastermind init` are required for this step.

## Install workflow adapters

```bash
mastermind install --client all
```

This installs Mastermind-owned skills for Claude and Codex, Claude subagents,
and both user-scope MCP registrations. Use `mastermind install` or
`mastermind install --client codex` for one client. Cursor and Continue expose
the MCP tools through `mastermind setup`; Mastermind does not claim a native
workflow-extension format for those clients.

The installer owns only artifacts recorded in a per-client manifest. Updates
replace each owned skill directory atomically, remove retired owned artifacts,
preserve unrelated user files, and roll back a partial client update.
Doctor hashes every owned artifact, so a locally modified or truncated skill is
reported as drift rather than accepted because the path merely exists.

```bash
mastermind list
mastermind update --client all
mastermind doctor --workflow --client all
```

## Use the codegraph without project initialization

From any supported code repository:

```bash
cd your-project
mastermind index .
mastermind status
mastermind map .
```

This creates `.mastermind/mmcg.db`. It does not create task specs, `CONTEXT.md`, or a workflow `CLAUDE.md`.

Refresh the index after edits:

```bash
mastermind index .       # incremental
mastermind index . --force
mastermind watch         # continuous refresh
```

Analyze the current worktree against a baseline:

```bash
mastermind impact --since main
```

## Enable the complete repository workflow

```bash
mastermind init
mastermind doctor
```

`init` scaffolds `.mastermind/`, builds the index, and creates project context
when missing. When invoked through the npm package, it also reconciles the
Claude workflow bundle unless `--no-global` is supplied. Existing `CONTEXT.md`
and `CLAUDE.md` files are preserved unless `--force` is used.

Useful opt-outs:

```bash
mastermind init --no-claude  # do not draft context through Claude Code
mastermind init --no-index   # scaffold without indexing
mastermind init --no-global  # do not update ~/.claude
```

## Choose a workflow depth

Most small changes do not need a task spec:

```bash
mastermind impact --since main
# implement and test
mastermind impact --since main
```

Use the default verified contract for normal multi-file or delegated work:

```bash
mastermind new-spec "Add account recovery"
mastermind verify-spec .mastermind/tasks/001-add-account-recovery/spec.md
# approve Scope and Acceptance Criteria
mastermind run-task .mastermind/tasks/001-add-account-recovery/spec.md --pre-only
# hand spec.md to the implementation agent; it writes executor-report.md
mastermind run-task .mastermind/tasks/001-add-account-recovery/spec.md --post-only
```

Use `--mode strict` for auth, billing, migrations, public APIs, data-loss,
supply-chain, or difficult rollback. `lite` and `standard` remain compatible
with old task files but are not recommended for new work.

`run-task` stores lifecycle state beside each canonical spec as
`<task>/state.json`. The executor owns only `<task>/executor-report.md`; the
controller owns state, `audit.md`, and release-note eligibility. The default
handoff is client-neutral. `--exec` is a legacy Claude CLI convenience.

## State and privacy

| Location | Contains | Commit it? |
|---|---|---|
| `~/.claude/` | Installed Claude agents, skills, ownership manifest | No |
| `~/.codex/` | Installed Codex skills and ownership manifest | No |
| User client config | MCP registration | No |
| `.mastermind/mmcg.db` | Local codegraph and scratchpad | No |
| `.mastermind/tasks/` | Optional specs, executor reports, audits, task state, and lessons | Your choice |
| `CONTEXT.md`, `CLAUDE.md` | Optional repository guidance | Your choice |

The index is generated from local source files and stays local. Add `.mastermind/` to `.gitignore` when you do not intend to version task artifacts.

## Next steps

- Run `mastermind map .` to learn a repository.
- Run `mastermind demo hallucinated-symbol` for a zero-setup audit example.
- Run `mastermind impact --since main` before submitting a change.
- Read [How the workflow works](workflow.md) to use checked task specs.
- Add [verifiable audits](github-action.md) to CI.
