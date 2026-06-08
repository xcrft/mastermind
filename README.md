# Mastermind

<p align="center">
  <img src="docs/assets/banner.webp" alt="mastermind — circuit-board logo, xcrft/mastermind" width="720">
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@xcraftmind/mastermind"><img src="https://img.shields.io/npm/v/@xcraftmind/mastermind.svg" alt="npm version"></a>
  <a href="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml"><img src="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

**mmcg** indexes your codebase into a local SQLite graph and serves it to Claude Code over MCP. Built on top: a spec-driven workflow where every structural claim an agent makes is verified against that graph — not the model's memory.

## Quick start

Requires Node.js 18+. No Rust toolchain.

```bash
npm install -g @xcraftmind/mastermind

cd your-project
mastermind init                        # build the index, scaffold .mastermind/, draft CONTEXT.md
mastermind setup claude --write-mcp    # register MCP once globally
mastermind doctor                      # verify — should be all green
```

Restart Claude Code. Ask it "who calls `parseConfig`?" — the answer comes from the codegraph, not a grep.

[Other install methods ↓](#install)

## The core: mmcg

mmcg is a Rust binary that tree-sitter-parses nine languages (Python, TypeScript/TSX, JavaScript/JSX, Rust, C#, Go, Java, PHP, C/C++) into a local SQLite database and answers structural questions over MCP:

| Question | Tool |
|---|---|
| Does symbol X exist? | `mmcg_search` |
| What calls X? | `mmcg_callers` |
| Transitive blast radius of changing X? | `mmcg_impact` |
| What files import this path? | `mmcg_imported_by` |
| What symbols did this branch add vs main? | `mmcg_symbols_changed_since` |
| Architectural cycles? | `mmcg_dependency_cycles` |
| Dead-code candidates? | `mmcg_unreferenced` |

Full 20-tool reference: [`mcp/servers/mmcg/README.md`](mcp/servers/mmcg/README.md).

**Why not grep / LSP / ast-grep / Sourcegraph?**

| Alternative | Gap |
|---|---|
| grep / ripgrep | text matching — no symbol graph, no caller semantics |
| ast-grep | good syntax matching, no persistent multi-language graph |
| LSP | strong semantics, harder to snapshot/query programmatically from an agent |
| Sourcegraph | heavier infrastructure |
| prompt-only workflows | no deterministic verification — claims about code are ungrounded |

mmcg is intentionally narrow: nine languages, 20 tools that map directly to what a coding agent needs, read-only over MCP. Sub-millisecond queries, zero system dependencies, no daemon.

## The workflow layer

Built on mmcg: a spec-driven pipeline where the planner never implements and the executor never improvises.

```mermaid
flowchart TB
  U([User]) --> P[Planner • Opus]
  P -.->|stress-test design| C[Critic • Opus]
  C -.->|7-dim verdict| P
  P -.->|need facts| R[Researcher • Haiku]
  R -.->|citations| P
  P -->|spec| E[Executor • Sonnet]
  E -->|report| A[Auditor • Opus]
  A -.->|contract held / drift / broken| P
  P -.->|on user request| Rel[Release • Sonnet]

  M[(mmcg codegraph)]
  P --- M
  E --- M
  A --- M
  R --- M
```

The critic (pre-spec) and auditor (post-execution) run as independent Opus instances with no prior conversation context — they catch the planner's bias.

### Mechanical gates

Every task spec has mandatory sections (Tests / Docs / Observability / Performance) enforced by two gates:

- **`mastermind verify-spec`** — pre-execution: mandatory sections present, every symbol in the spec exists in the index, all files on disk, FIND blocks not stale, PATH commands resolvable. Fails the spec before the executor touches code.
- **`mastermind audit-spec`** — post-execution: scope creep, signature drift vs the pre-edit snapshot, removed symbols not acknowledged, planned tests not added.
- **`mastermind run-task`** — wires the two gates around executor hand-off with state persistence between phases.

Full 14-step flow: the [workflow CLAUDE.md template](agents/claude-md/mastermind-workflow.md).

### Engineering canons

Enforced by critic and auditor on every task:

- **No AI slop** — every structural claim grounded in mmcg, not memory. Hallucinated symbols and fabricated SLAs are flagged.
- **YAGNI** — no speculative features or premature abstractions.
- **Non-breaking** — public-API changes require explicit justification + deprecation path in the spec.
- **Mandatory spec sections** — Tests / Documentation / Observability / Performance. Empty sections fail pre-flight.
- **Pre-edit snapshot** — planner records `mmcg_callers` counts + signatures before editing; auditor diffs post-execution to surface silent breakage.

## What's inside

```
Core
  mcp/servers/mmcg/     Rust codegraph binary (mmcg) — 20 MCP tools, 9 languages

Workflow pack (installed into ~/.claude/ by `mastermind init`)
  agents/               7 subagents: critic, auditor, researcher, task-executor,
                        release, prompt-refiner + CLAUDE.md / CONTEXT.md templates
  skills/               Workflow skills (mastermind-task-planning, mastermind-task-executor)
                        and standalone skills (pr-review, flaky-finder, doc-stub-sync)
  prompts/              Reusable prompt templates (api-shape-explorer)
```

Each folder is grouped by domain inside (e.g. `skills/code-review/`); every category ships a `_template/` to copy when adding something new. The naming and frontmatter standard lives in [`docs/conventions.md`](docs/conventions.md).

## Install

Mastermind ships as a **prebuilt native binary via npm** — no Rust toolchain needed.

**Global** (recommended)

```bash
npm install -g @xcraftmind/mastermind
mastermind setup claude --write-mcp
```

Registers `command: "mastermind"` at Claude Code user scope via `claude mcp add --scope user` (writes `~/.claude.json`). Refuses to re-register without `--force`.

**Project-local** (version-pinned)

```bash
npm install -D @xcraftmind/mastermind
npx mastermind setup claude --project . --write-mcp
```

Writes `./.mcp.json` with `command: "./node_modules/.bin/mastermind"`.

**One-shot** — `npx -y @xcraftmind/mastermind doctor`. Fine for one-offs; avoid for the MCP server (re-resolves through npm cache on every Claude Code launch).

**Supported platforms** — macOS (arm64 + x86_64), Linux glibc & musl/Alpine (x86_64 + arm64), Windows (x86_64). Other targets: `cargo install mmcg`.

<details>
<summary>Claude Code plugin marketplace</summary>

```bash
/plugin marketplace add xcrft/mastermind
/plugin install mastermind-workflow@mastermind   # workflow subagents
/plugin install mmcg@mastermind                  # codegraph MCP config
/plugin install mastermind-tools@mastermind      # standalone skills
```

The `mmcg` plugin registers the MCP config; install the binary via `npm install -g @xcraftmind/mastermind`. The `plugins/` tree is generated from canonical artifacts by [`scripts/build-plugins.py`](scripts/build-plugins.py).
</details>

<details>
<summary>Build from source</summary>

```bash
cargo install mmcg                       # crates.io
# or from a clone:
cargo install --path mcp/servers/mmcg
```

Requires Rust 1.75+ ([rustup](https://rustup.rs/)). The binary is `mmcg` — same code, same subcommands.
</details>

## Use it in a project

```bash
mastermind init                  # scaffold .mastermind/, build the index, draft CONTEXT.md
echo .mastermind/ >> .gitignore  # local working state — never committed
mastermind doctor                # 8 fail-soft checks: binary, index, freshness, MCP config, serve handshake, …
```

`init` indexes the codebase and drafts `CONTEXT.md` via `claude -p` — pass `--no-index` / `--no-claude` to skip, or `--with-claude-md` to also write the workflow CLAUDE.md template. It installs the workflow subagents, skills, and slash commands into `~/.claude/` (`--no-global` to skip). Keep the index live with `mastermind watch`. Tear a setup down with `mastermind uninstall` (`--scope project|global|all`; dry-run unless `--force`).

`mastermind doctor --json` is CI-friendly (exit 1 if anything's unwired).

## What `init` does — and what it doesn't

Nothing contacts an external service unless the Claude Code CLI is installed and `--no-claude` is not passed.

**Created locally** (always, no network):

| Path | What it is |
|---|---|
| `.mastermind/` | Index directory. `.gitignore` inside wildcards everything so it's never committed. |
| `.mastermind/tasks/` | Where your task specs live. |
| `.mastermind/mmcg.db` | SQLite codegraph index — built locally from your source files by tree-sitter. Never leaves your machine. |
| `CONTEXT.md` | Written from a template. Not overwritten if it already exists (unless `--force`). |

**Installed globally** into `~/.claude/` (npm installs only; skippable with `--no-global`):

Subagent `.md` files → `~/.claude/agents/`, skill directories → `~/.claude/skills/`, slash-command files → `~/.claude/commands/`. These are static Markdown files. No code runs during the copy.

**What goes to Claude** (only if the Claude Code CLI is installed and `--no-claude` is not passed):

`claude -p "<prompt>" --permission-mode acceptEdits` is invoked once to fill `CONTEXT.md` (and optionally `CLAUDE.md`) placeholders. The prompt asks Claude to read the codebase using MCP tools and fill only specific sections — it does not batch-upload raw source files. If the `claude` binary is not on PATH, this step is skipped with a warning and the templates are left unfilled.

`mastermind setup claude --write-mcp` runs `claude mcp add --scope user mmcg -- mastermind serve`, which modifies `~/.claude.json`. That is the only file outside your project that `setup` touches.

**Fully offline / no-LLM path:**

```bash
mastermind init --no-claude --no-index --no-global
```

Scaffolds `.mastermind/` and writes templates. Zero network calls, zero subprocesses.

**Remove everything:**

```bash
mastermind uninstall --scope all --force
# also remove the workflow files installed into ~/.claude/:
rm ~/.claude/agents/mastermind-*.md
rm -rf ~/.claude/skills/mastermind-*/
rm ~/.claude/commands/mastermind-*.md
```

`CONTEXT.md` and `CLAUDE.md` are never touched by `uninstall` — they are your project's files.

---

## Just the codegraph

`mmcg` is a standalone [crate](https://crates.io/crates/mmcg) that works with any MCP stdio client (Cursor, Continue, custom) — 20 tools across nine languages: `search`, `callers`, `callees`, `impact`, `imports`, `imported_by`, `files`, `outline`, `symbols_in_file`, `recent_changes`, `unreferenced`, `api_surface`, `centrality`, `tasks`, `dependency_cycles`, `symbols_changed_since`, `scratchpad_append`, `scratchpad_read`, `change_class`, `status`.

```bash
cargo install mmcg
```

Register it in your MCP client — snippet in [`mcp/servers/mmcg/README.md`](mcp/servers/mmcg/README.md).

The subagent files use Claude Code's frontmatter format, but individual pieces are portable. A lone critic agent without the full pipeline is still a fine code reviewer; spec-driven contracts need the planner + auditor too.

## Contributing

Before adding anything, read:

- [`docs/conventions.md`](docs/conventions.md) — naming, frontmatter, file layout. **This is the standard.**
- The `*-anatomy.md` for your artifact type (e.g. [`docs/skill-anatomy.md`](docs/skill-anatomy.md)).
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — the PR process.

Copy the matching `_template/`, fill it in, open a PR.

## License

MIT — see [`LICENSE`](LICENSE).
