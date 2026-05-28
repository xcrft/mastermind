# @xcraftmind/mastermind

[![npm version](https://img.shields.io/npm/v/@xcraftmind/mastermind.svg)](https://www.npmjs.com/package/@xcraftmind/mastermind)
[![CI](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg)](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml)
[![license: MIT](https://img.shields.io/npm/l/@xcraftmind/mastermind.svg)](./LICENSE)

Mastermind workflow CLI + mmcg codegraph for AI coding agents — verify-spec / audit-spec gates, MCP server, multi-language tree-sitter indexer (Python, TypeScript, JavaScript, Rust, C#, Go, Java, PHP, C/C++).

Prebuilt native binaries via optional platform packages — **no Rust toolchain required**.

## What it does

`mastermind` parses your codebase into a queryable graph (definitions, callers/callees, imports, blast radius) with tree-sitter, and serves it to Claude Code over MCP — so the agent asks structural questions instead of grepping. It also ships the Mastermind workflow gates: `verify-spec` (pre-flight) and `audit-spec` (post-flight) for running coding tasks against mechanical checks.

## Quick start

Requires **Node.js 18+**. The CLI is a thin JS wrapper over a prebuilt native binary — no Rust toolchain needed.

**1. Install**

```sh
npm install -g @xcraftmind/mastermind
```

**2. Set up your project** — run inside each repo you want Claude to understand:

```sh
cd your-project
mastermind init                      # scaffold .mastermind/, build the index, draft CONTEXT.md
echo ".mastermind/" >> .gitignore    # index + local specs are local state
```

`init` builds the index and drafts `CONTEXT.md` from your code via `claude -p` (pass `--no-claude` or `--no-index` to skip). It also installs the workflow subagents, skills, and slash commands into `~/.claude/` so the full pipeline (planner / critic / executor / auditor) is available, not just the codegraph (`--no-global` to skip). Re-run `mastermind index .` to refresh, or `mastermind watch` to keep it live.

**3. Register with Claude Code** — once, globally:

```sh
mastermind setup claude --write-mcp
```

**4. Verify**

```sh
mastermind doctor                    # should now be all green
```

Restart Claude Code — the codegraph tools (search, callers, callees, impact, …) are now available in any project you've indexed.

## What gets set up where

Three pieces — the split is the part that trips people up:

| | Scope | Lives in | How often |
|---|---|---|---|
| **Index** — `init` + `index` | **per project** | `.mastermind/mmcg.db` in each repo | once per repo, refresh with `index` / `watch` |
| **Workflow** — subagents, skills, commands | global | `~/.claude/{agents,skills,commands}/` | installed + refreshed by `init` |
| **MCP registration** — `setup claude` | once | `~/.claude/.mcp.json` | once for all projects |

- **The index is always per-project.** Run `mastermind init` in *every* repo you want indexed. `doctor` reporting `index database not found` just means you haven't done this in the current directory yet (the exact situation if you run `doctor` from `/tmp` or a fresh shell).
- **The workflow installs globally on `init`** — subagents, skills + slash commands land in `~/.claude/{agents,skills,commands}/`, overwriting Mastermind's own files to keep them current (`--no-global` to skip). Ships with the npm package; cargo installs use the plugin marketplace instead.
- **The MCP registration is usually once, globally.** The global entry launches `mastermind serve` from whichever project you open in Claude Code, so it picks up *that* project's `.mastermind/mmcg.db` automatically. You do **not** re-run `setup claude` per repo.
- Use **per-project registration** only if you want the MCP config committed with the repo and version-pinned: `mastermind setup claude --project . --write-mcp` writes `./.mcp.json` with `command: "./node_modules/.bin/mastermind"` (pair it with a project-local install — see below).

> `setup claude` is safe by default: without `--write-mcp` it prints the diff and exits without touching anything.

## Install options

### Global (recommended)

```sh
npm install -g @xcraftmind/mastermind
```

Puts `mastermind` on your PATH. `setup claude --write-mcp` registers `command: "mastermind"` in `~/.claude/.mcp.json`.

### Project-local

```sh
npm install -D @xcraftmind/mastermind
npx mastermind setup claude --project . --write-mcp
```

Writes `./.mcp.json` with `command: "./node_modules/.bin/mastermind"` — reproducible and version-pinned with the repo. Commit `./.mcp.json`; keep ignoring `.mastermind/`.

### One-shot with npx (no install)

```sh
npx -y @xcraftmind/mastermind doctor
npx -y @xcraftmind/mastermind init --profile typescript-api
```

Fine for one-off commands. **Avoid npx for the MCP server** — running `npx … serve` on every Claude Code launch pays an npm-resolution cost and is less deterministic than a real install. For `serve` / `setup claude`, prefer global or project-local.

### Build from source (contributors / unsupported platforms)

```sh
cargo install mmcg     # requires Rust 1.75+
```

The cargo-installed binary is `mmcg`, not `mastermind` — same code, same subcommands, only the wrapper name differs.

## Commands

```sh
mastermind init [--profile <p>]             # scaffold .mastermind/, build index, draft CONTEXT.md (--no-index / --no-claude to skip)
mastermind index .                          # build/refresh the codegraph (incremental; --force to re-parse all)
mastermind watch                            # long-running watcher — re-indexes on file changes
mastermind status                           # file count, symbol count, db path
mastermind doctor                           # environment health check
mastermind serve                            # MCP stdio server (this is what Claude Code launches)
mastermind setup claude --write-mcp         # register with Claude Code's MCP layer
mastermind verify-spec <path>               # pre-execution mechanical gate on a task spec
mastermind audit-spec <path> --since main   # post-execution audit vs a git baseline
mastermind run-task <path>                  # two-phase orchestrator: verify → executor → audit
mastermind query callers <symbol>           # one-shot CLI query (agents use the MCP tools instead)
mastermind uninstall [--scope <s>]          # remove project setup (.mastermind/ + project .mcp.json); --scope global|all for the global MCP entry
```

`mmcg` is a legacy alias for the same binary (cargo installs expose it under that name) — prefer `mastermind`. See `mastermind <subcommand> --help` for full options.

## Supported platforms

Prebuilt binaries ship for:

| Platform | Architecture |
|---|---|
| macOS (Apple Silicon) | aarch64 |
| macOS (Intel) | x86_64 |
| Linux glibc | x86_64, aarch64 |
| Linux musl (Alpine) | x86_64, aarch64 |
| Windows | x86_64 |

Other targets fall back to `cargo install mmcg`.

## Architecture

The npm package uses the prebuilt-platform-package pattern (same as `esbuild`, `swc`, `lefthook`, `turbo`). The root `@xcraftmind/mastermind` package contains only JavaScript wrappers and lists all seven platform packages as `optionalDependencies`. npm installs only the package matching the host's `os` + `cpu` (+ `libc` for Linux); the others are skipped.

```
@xcraftmind/mastermind                    # JS wrappers + optionalDependencies
├── @xcraftmind/mmcg-darwin-arm64         # one of these installs, the rest skip
├── @xcraftmind/mmcg-darwin-x64
├── @xcraftmind/mmcg-linux-x64-gnu
├── @xcraftmind/mmcg-linux-arm64-gnu
├── @xcraftmind/mmcg-linux-x64-musl
├── @xcraftmind/mmcg-linux-arm64-musl
└── @xcraftmind/mmcg-win32-x64-msvc
```

No `postinstall` script. No network calls beyond the npm registry itself.

## Links

- Source: [github.com/xcrft/mastermind](https://github.com/xcrft/mastermind)
- Changelog: [CHANGELOG.md](https://github.com/xcrft/mastermind/blob/main/CHANGELOG.md)
- mmcg Rust crate: [crates.io/crates/mmcg](https://crates.io/crates/mmcg)
- MCP protocol: [modelcontextprotocol.io](https://modelcontextprotocol.io)

## License

MIT — see [LICENSE](./LICENSE).
