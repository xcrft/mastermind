# Getting started

Install the CLI, index a repository, and inspect a change. Task control and
personal-profile mining are optional.

## 1. Install

| Method | Requirement | Commands |
|---|---|---|
| Global npm | Node.js 24+ | `mastermind`, `mmcg` |
| Project npm | Node.js 24+ | `npx mastermind` |
| Cargo | Rust 1.96+ | `mmcg` |

Sources: [npm manifest](../npm/mastermind/package.json),
[Cargo manifest](../mcp/servers/mmcg/Cargo.toml).
npm ships native macOS, Linux, and Windows binaries.

```bash
npm install -g @xcraftmind/mastermind
mastermind --version
```

Alternatives:

```bash
npm install -D @xcraftmind/mastermind
npx mastermind --version
```

```bash
cargo install mmcg --locked
mmcg --version
```

## 2. Index your repository

```bash
cd your-repository
mastermind index .
mastermind status
```

The index is `.mastermind/mmcg.db`. Add `.mastermind/` to `.gitignore`:

```gitignore
.mastermind/
```

| Action | Command | Result |
|---|---|---|
| Refresh | `mastermind index .` | Re-index changed files |
| Rebuild | `mastermind index . --force` | Reparse all discovered files |
| Watch | `mastermind watch` | Refresh while the process runs |
| Inspect | `mastermind status` | Freshness and workflow blockers |

Discovery follows ignore rules. Code, documentation, and history have separate
freshness checks. Resolve warnings before relying on their results.

## 3. Review the current change

```bash
mastermind impact --since main
mastermind ui --since main
```

Replace `main` with an existing Git baseline. Staged, unstaged, and untracked
changes count.

| Lens view | Shows |
|---|---|
| Review | Changed symbols, callers, boundaries, and test candidates |
| Audit | Surrounding architecture |
| Profiles | Selected person, project, code, documentation, and work layers |

Profiles shows missing or withheld data explicitly. Its private personal layer
requires a grant and stays out of standalone exports.

| Research need | Command |
|---|---|
| Repository map | `mastermind map .` |
| Compact code briefing | `mastermind brief --role planner --since main --budget-tokens 2000` |
| Combined context | `mastermind context preview --role planner --since main --query "service boundaries"` |

Previews show source revisions and omissions. They do not establish model delivery.

## 4. Connect a coding client

```bash
mastermind install --client all
mastermind doctor --workflow --client all
```

| Selection | Effect |
|---|---|
| `--client claude` | Claude Code workflow |
| `--client codex` | Codex portable skills |
| `--client all` | Both |
| `--profile core` | Default skill selection |
| `--profile frontend`, `security`, `full` | Additional skills |

Updates retain the selected profile unless you change it. For MCP alone:

```bash
mastermind setup claude --scope user
mastermind setup claude --scope user --write
```

The first command previews configuration. Restart the client after applying it.
See [Claude Code](integrations/claude-code.md), [Codex](integrations/codex.md),
[Cursor](integrations/cursor.md), [Continue](integrations/continue.md), or
[generic MCP](integrations/generic-mcp.md).

## Optional: use task control

```bash
mastermind init --no-claude --no-global
mastermind new-spec "Add account recovery"
```

| Option | Effect |
|---|---|
| `--no-claude` | Leave context drafting local, without a model call |
| `--no-global` | Skip reconciliation into `~/.claude/` |
| Default file handling | Preserve existing `CONTEXT.md` and `CLAUDE.md` |
| `--force` | Replace existing generated guidance |

Fill the generated spec before approval. Follow [Workflow](workflow.md) through
checks, audit, review, and completion. Indexing and MCP do not require `init`.

## Optional: build a personal profile

[Persona hooks](guides/persona-hooks.md) covers capture, analysis, authorship
review, and profile access. Workflow installation enables neither capture nor
global-profile reading.

## Optional: export a review

```bash
mastermind review export --since main --out mastermind-review
```

The new directory contains offline HTML, SARIF, a summary, an evidence manifest,
and a GitHub Actions workflow. Existing output paths are rejected.
See the [export reference](reference/mmcg.md#pr-evidence-package-mmcg-review-export).

## Storage and provider access

| Location | Contents |
|---|---|
| `.mastermind/mmcg.db` | Repository index and scratchpad |
| `.mastermind/tasks/` | Specs, checks, execution, audit, and review records |
| `CONTEXT.md` | Maintained project knowledge |
| `~/.mastermind/` | Optional global personal profile and capture journal |
| Client configuration | MCP registration and workflow adapters |

| Operation | Model access |
|---|---|
| Indexing, deterministic queries, Lens, export | None |
| `init` without `--no-claude` | Claude-assisted context drafting |
| Native execution and review | Configured Claude provider |
| AI mining | Explicitly selected processor or provider |

## Update

```bash
npm install -g @xcraftmind/mastermind@latest
mastermind update --client all
mastermind doctor --workflow --client all
```

Use `--profile` only to change the installed selection.
Use the client guide for scope-specific removal.
