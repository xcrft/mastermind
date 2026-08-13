# Getting started

This guide takes a clean machine to a local index, a change-impact report, and
an optional MCP connection. For exact flags and schemas, use the
[technical reference](reference/mmcg.md).

## Requirements

| Install path | Requirement |
|---|---|
| npm, recommended | Node.js 24+ |
| Cargo | Rust 1.96+ |
| Baseline-aware commands | Git |
| MCP | Claude Code, Codex, Cursor, Continue, or another stdio client |

The npm package includes native binaries for supported macOS, Linux, and
Windows targets. It does not compile Rust during installation.

## 1. Install the CLI

Global installation:

```bash
npm install -g @xcraftmind/mastermind
mastermind --version
```

Repository-pinned installation:

```bash
npm install -D @xcraftmind/mastermind
npx mastermind --version
```

Cargo installation exposes the same binary as `mmcg`:

```bash
cargo install mmcg --version 2.0.0 --locked
mmcg --version
```

## 2. Index a repository

```bash
cd your-repository
mastermind index .
mastermind status
```

The index is `.mastermind/mmcg.db`. Source discovery follows Git and `.ignore`
rules and skips build/vendor directories. The first run parses supported files;
later runs skip unchanged files.

```bash
mastermind index .          # incremental refresh
mastermind index . --force  # full reparse
mastermind watch            # long-running incremental refresh
```

Do not commit the database. `mastermind init` adds the normal ignore rule, but
an index created directly may require this repository-specific entry:

```gitignore
.mastermind/
```

## 3. Get the first useful result

```bash
mastermind map .
mastermind impact --since main
mastermind temporal --since main
mastermind ui --since main
```

- `map` summarizes components, entry points, dependencies, hotspots, and
  cycles.
- `impact` connects the current Git diff to changed symbols, callers,
  component crossings, and candidate tests.
- `temporal` compares architecture at the baseline and indexed worktree.
- `ui` serves the same bounded snapshot in local read-only Mastermind Lens.

Refresh the index before trusting a result after source changes. Stale,
truncated, or work-limited analysis fails closed or is labelled partial; it is
never presented as a clean review.

## 4. Connect an AI client

CLI use does not require MCP. If you want an agent to query the graph, choose
one setup path:

| Client | Command |
|---|---|
| Claude Code | `mastermind install` |
| Codex | `mastermind install --client codex` |
| Claude Code + Codex | `mastermind install --client all` |
| Cursor | `mastermind setup cursor --scope user --write` |
| Continue | `mastermind setup continue --scope user --write` |
| Generic stdio client | [Generic MCP guide](integrations/generic-mcp.md) |

`mastermind setup` previews a redacted plan when `--write` is absent. Client
guides describe project scope, config ownership, backups, updates, and removal:

- [Claude Code](integrations/claude-code.md)
- [Codex](integrations/codex.md)
- [Cursor](integrations/cursor.md)
- [Continue](integrations/continue.md)
- [Generic MCP](integrations/generic-mcp.md)

Verify installed workflow files and MCP configuration:

```bash
mastermind doctor --workflow --client all
```

## Optional: export review evidence

```bash
mastermind review export --since main --out mastermind-review
```

The new directory contains standalone HTML, SARIF, a Markdown summary, a
revision/evidence manifest, and a pinned GitHub Actions workflow. The exporter
does not overwrite an existing path.

## Optional: enable the task workflow

You do not need `mastermind init` for indexing, CLI analysis, Lens, or MCP.
Run it only when the repository should use Mastermind's task artifacts and
project context:

```bash
mastermind init --no-claude
mastermind doctor
```

Omit `--no-claude` only when you intend to let the configured Claude CLI draft
repository context from source content. Existing `CONTEXT.md` and `CLAUDE.md`
are preserved unless `--force` is supplied.

The task workflow has three depths:

| Mode | Use for | Task spec |
|---|---|---|
| Direct | Small, reversible changes | None |
| Verified | Normal multi-file or delegated work | Required |
| Strict | Auth, billing, migrations, public API, data loss, supply chain, difficult rollback | Required with risk and review evidence |

Read [Workflow](workflow.md) before using `new-spec`, `verify-spec`, or
`run-task`.

## State and network behavior

| Path | Scope | Contents | Commit? |
|---|---|---|---|
| `.mastermind/mmcg.db` | Repository | Generated graph and local scratchpad | No |
| `.mastermind/tasks/` | Repository | Optional specs, reports, audits, state | Project decision |
| `CONTEXT.md`, `CLAUDE.md` | Repository | Optional human/agent guidance | Project decision |
| `~/.mastermind/` | User | Optional style profile | No |
| Client config and workflow directories | User or repository | MCP registration and installed adapters | Depends on selected scope |

Indexing, queries, Lens, policy evaluation, fact import, and review export run
locally. The commands that may invoke an external model are explicit:

- `mastermind init` without `--no-claude`;
- `mastermind miner profile --deep`.

## Update or remove

```bash
npm install -g @xcraftmind/mastermind@latest
mastermind update --client all
mastermind doctor --workflow --client all
```

Removal is scope-specific and dry-run-first. Read the relevant client guide and
inspect the printed plan before adding a destructive flag.

## Next

- [Documentation index](README.md)
- [CLI and MCP reference](reference/mmcg.md)
- [Benchmarks](benchmarks.md)
- [GitHub Action](github-action.md)
