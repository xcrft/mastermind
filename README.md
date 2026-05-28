# Mastermind

<p align="center">
  <img src="docs/assets/banner.webp" alt="mastermind — circuit-board logo, xcrft/mastermind" width="720">
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@xcraftmind/mastermind"><img src="https://img.shields.io/npm/v/@xcraftmind/mastermind.svg" alt="npm version"></a>
  <a href="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml"><img src="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
</p>

A curated standard library of **skills**, **prompts**, **agent configs**, and **MCP integrations** for AI coding agents — primarily [Claude Code](https://claude.com/claude-code), portable to any MCP client.

Headlined by a **spec-driven planner / critic / executor / auditor pipeline** grounded in a real codegraph (**mmcg**): every claim a subagent makes about your code is verified against an AST, not hallucinated.

## Quick start

```bash
npm install -g @xcraftmind/mastermind     # prebuilt native binary — no Rust toolchain

cd your-project
mastermind init                            # scaffold .mastermind/, build the index, draft CONTEXT.md
mastermind setup claude --write-mcp        # register the codegraph with Claude Code (run once)
mastermind doctor                          # verify — should be all green
```

Restart Claude Code and the codegraph tools (search, callers, impact, …) are available in every project you've indexed. Requires Node.js 18+. [Other install methods ↓](#install)

## Why mastermind

Most "AI workflow" templates are a planner prompt and good wishes. Here, engineering canons — YAGNI, non-breaking, no AI slop, mandatory Tests / Docs / Observability / Performance sections in every spec — are enforced by an independent Opus **critic** before a spec leaves the planner, and again by an independent Opus **auditor** after the executor reports. Every structural claim is checked against the **mmcg** codegraph, not the model's memory.

## What's inside

| Folder | What it holds |
|---|---|
| [`skills/`](skills/) | Markdown skills with frontmatter — workflow skills (`mastermind-task-planning`, `mastermind-task-executor`, `mastermind-incident-response`, `mastermind-prompt-refiner`) and standalone ones (`pr-review`, `flaky-finder`, `doc-stub-sync`). |
| [`prompts/`](prompts/) | Reusable system/user prompt templates (`api-shape-explorer`). |
| [`agents/`](agents/) | Six mastermind subagents (`critic`, `auditor`, `researcher`, `task-executor`, `release`, `prompt-refiner`) plus `CLAUDE.md` / `CONTEXT.md` templates. |
| [`mcp/`](mcp/) | MCP server configs, headlined by **`mmcg`** — a fast Rust codegraph indexer (Python, TS/JS, Rust, C#, Go, Java, PHP, C/C++) with 17 tools. |

Each folder is grouped by **domain** inside (e.g. `skills/code-review/`); every category ships a `_template/` to copy when adding something new. The standard itself lives in [`docs/conventions.md`](docs/conventions.md).

## How it works

A task flows from the user through 4–7 specialized subagents — each at its own model tier — with **mmcg** as the shared truth source.

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

The **planner** never implements. The **executor** never improvises. The **critic** (pre-spec) and the **auditor** (post-execution) run as independent Opus instances with no prior conversation context — they catch the planner's bias.

Underneath every role sits **mmcg**: a Rust binary that indexes nine languages into a local SQLite codegraph and answers structural questions over MCP — *"does function X exist?"*, *"who calls Y?"*, *"transitive blast radius of changing Z?"*, *"what symbols did this branch add since main?"*. That grounding is what makes the workflow's claims resistant to hallucination. Full 14-step flow: the [workflow CLAUDE.md template](agents/claude-md/mastermind-workflow.md).

## Engineering canons

Enforced by the critic (pre-spec) and the auditor (post-execution):

- **No AI slop** — every claim grounded in code via mmcg, not memory. Hallucinated symbols, fabricated SLAs, and padded "best practices" are flagged.
- **YAGNI** — no speculative features, premature abstractions, or future-proofing for unstated requirements.
- **Non-breaking** — public-API changes require an explicit justification + deprecation path in the spec.
- **Mandatory spec sections** — Tests / Documentation / Observability / Performance. Empty sections fail pre-flight.
- **Pre-edit symbol snapshot** — the planner records `mmcg_callers` counts + signatures before editing; the auditor diffs post-execution to surface silent breakage.

## Install

Mastermind ships as a **prebuilt native binary via npm** — no Rust toolchain on the normal path. `mastermind` is the command; `mmcg` is the same binary under its cargo name.

**Global** (recommended)

```bash
npm install -g @xcraftmind/mastermind
mastermind setup claude --write-mcp
```

Writes `~/.claude/.mcp.json` with `command: "mastermind"`. Merges into an existing `mcpServers` and refuses to clobber a customized entry.

**Project-local** (reproducible, version-pinned)

```bash
npm install -D @xcraftmind/mastermind
npx mastermind setup claude --project . --write-mcp
```

Writes `./.mcp.json` with `command: "./node_modules/.bin/mastermind"`.

**One-shot** — `npx -y @xcraftmind/mastermind doctor`. Fine for one-offs; avoid it for the MCP server (it re-resolves through the npm cache on every Claude Code launch).

**Supported platforms** — macOS (arm64 + x86_64), Linux glibc & musl/Alpine (x86_64 + arm64), Windows (x86_64). Other targets fall back to `cargo install mmcg`.

<details>
<summary>Claude Code plugin marketplace</summary>

```bash
/plugin marketplace add xcrft/mastermind
/plugin install mastermind-workflow@mastermind   # workflow subagents
/plugin install mmcg@mastermind                  # codegraph MCP config
/plugin install mastermind-tools@mastermind      # standalone skills
```

The `mmcg` plugin registers the MCP config; install the binary via `npm install -g @xcraftmind/mastermind`. The `plugins/` tree is regenerated from canonical artifacts by [`scripts/build-plugins.py`](scripts/build-plugins.py).
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

From any project's working directory:

```bash
mastermind init                  # scaffold .mastermind/, build the index, draft CONTEXT.md via claude -p
echo .mastermind/ >> .gitignore  # local working state — never committed
mastermind doctor                # 8 fail-soft checks: binary, index, freshness, MCP config, serve handshake, …
```

`init` indexes the codebase and drafts `CONTEXT.md` automatically — pass `--no-index` / `--no-claude` to skip, or `--with-claude-md` to also drop the workflow CLAUDE.md template. Keep the index live with `mastermind watch`, and tear a setup down with `mastermind uninstall` (`--scope project|global|all`; dry-run unless `--force`).

`.mastermind/` holds the project's specs and the mmcg index. `mastermind doctor --json` is CI-friendly (exit 1 if anything's unwired).

## Subsets (advanced)

**Just the codegraph** — `mmcg` is a standalone [crate](https://crates.io/crates/mmcg): 17 tools (`search`, `callers`, `callees`, `impact`, `imports`, `imported_by`, `files`, `outline`, `symbols_in_file`, `recent_changes`, `unreferenced`, `api_surface`, `centrality`, `tasks`, `dependency_cycles`, `symbols_changed_since`, + a `status` diagnostic) across nine languages. Sub-millisecond queries, zero system deps.

```bash
cargo install mmcg
```

Register it in your MCP client — snippet in [`mcp/servers/mmcg/README.md`](mcp/servers/mmcg/README.md).

**Just specific subagents or skills** — copy the markdown into `~/.claude/`:

```bash
cp agents/subagents/mastermind-critic.md ~/.claude/agents/
cp -r skills/code-review/pr-review ~/.claude/skills/
```

The pipeline's guarantees hold only with the full set — a lone critic is still a fine code reviewer, but the spec-driven contracts need the planner + auditor too.

## Not using Claude Code?

`mmcg` is tool-agnostic — it works with any MCP stdio client (Cursor, Continue, custom). The subagent files use Claude Code's frontmatter format, but the underlying patterns (the pipeline, the spec template, the engineering canons) are portable.

## Contributing

The repo lives or dies by consistency. Before adding anything, read:

- [`docs/conventions.md`](docs/conventions.md) — naming, frontmatter, file layout. **This is the standard.**
- The `*-anatomy.md` for your artifact type (e.g. [`docs/skill-anatomy.md`](docs/skill-anatomy.md)).
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — the PR process.

To add a new artifact: copy the matching `_template/`, fill it in, open a PR.

## License

MIT — see [`LICENSE`](LICENSE).
