# Getting started

In a few minutes you will have a local codegraph, a blast-radius report for the
current change, and—if you want it—an MCP connection for your coding agent.
Your repository stays on the machine.

For exact flags, schemas, and limits, use the
[technical reference](reference/mmcg.md). This page is the shortest path to a
useful result.

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

Choose one installation path. npm is the fastest route because it ships a
prebuilt native binary.

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
cargo install mmcg --version 2.1.0 --locked
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

You are ready when `mastermind status` reports the repository and indexed file
counts without stale-file warnings.

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

For a first review, run `impact`, then open `ui`. Use `map` when you need the
architecture around the change and `temporal` when you need base-versus-head
drift.

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

Fresh installs use `core`. Select a larger portable-skill catalog only when the
repository needs it:

| Profile | Skills | Focus |
|---|---:|---|
| `core` | 14 | Planning, codegraph research, change/test impact, execution, and comment/test audit |
| `frontend` | 19 | `core` plus component, design, browser, frontend, and runtime research |
| `security` | 17 | `core` plus security research, agent review, and audit attestation |
| `full` | 26 | Every shipped portable skill |

```bash
mastermind install --client all --profile frontend
mastermind list --profile frontend
```

Profiles narrow skill discovery, not Claude's named subagent routes. All Claude
subagents remain installed. An existing installation keeps its recorded
profile when `install`, `update`, or `doctor --workflow` omits `--profile`.
Legacy manifests from before profiles are treated as `full`, then upgraded
without silently retiring skills.
Manifest schema downgrades are unsupported. An older installer rejects a
schema-v2 manifest before replacing managed files, so use the current package
for later updates.

If final cleanup fails after a committed install, the command succeeds with a
`cleanup pending` warning. The reported staging directory keeps the pre-update
backups. Keep it until `mastermind doctor --workflow` passes, then remove that
exact directory.

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

Pass `--profile core|frontend|security|full` to `update` only when you intend to
switch profiles. Profile changes reconcile Mastermind-owned artifacts and leave
unrelated client files untouched.

Removal is scope-specific and dry-run-first. Read the relevant client guide and
inspect the printed plan before adding a destructive flag.

## Next

- [Documentation index](README.md)
- [CLI and MCP reference](reference/mmcg.md)
- [Benchmarks](benchmarks.md)
- [GitHub Action](github-action.md)
