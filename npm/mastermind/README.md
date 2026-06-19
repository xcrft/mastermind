# @xcraftmind/mastermind

[![npm](https://img.shields.io/npm/v/@xcraftmind/mastermind.svg)](https://www.npmjs.com/package/@xcraftmind/mastermind)
[![downloads](https://img.shields.io/npm/dm/@xcraftmind/mastermind.svg)](https://www.npmjs.com/package/@xcraftmind/mastermind)
[![node](https://img.shields.io/node/v/@xcraftmind/mastermind.svg)](https://nodejs.org)
[![license: MIT](https://img.shields.io/npm/l/@xcraftmind/mastermind.svg)](./LICENSE)

A queryable codegraph for Claude Code, plus a plan → execute → audit workflow with mechanical gates. Prebuilt native binaries — **no Rust toolchain required.**

`mmcg` parses your repo with tree-sitter into a graph of definitions, callers/callees, imports, and blast radius, and serves it to Claude Code over MCP — so the agent queries structure instead of grepping. The package also installs the Mastermind workflow (planner / critic / executor / auditor subagents + skills) and the `verify-spec` / `audit-spec` gates that check coding tasks against the actual diff.

**Languages:** Python, TypeScript, JavaScript, Rust, C#, Go, Java, PHP, C/C++.

## Requirements

- **Node.js 24+**
- **Claude Code** — for the MCP server and the workflow agents

## Quick start

```sh
npm install -g @xcraftmind/mastermind
mastermind install                  # workflow agents + skills + MCP → Claude Code (global, once)
cd your-project && mastermind init  # build the codegraph index for this repo
```

Restart Claude Code. The agent can now answer structural questions (`who calls parseConfig?`, blast radius of a change) and run the planning workflow against your code.

> **Two scopes — the part that trips people up.** The **index is per-project**: `.mastermind/mmcg.db` in each repo, built by `mastermind init` (run it in every repo you want indexed). The **workflow agents/skills + MCP registration are global** (`~/.claude/`): `mastermind install` sets them up once for all projects.

## Commands

```sh
mastermind install                          # copy workflow agents + skills into ~/.claude + register the MCP server
mastermind update                           # refresh the agents + skills (MCP already registered)
mastermind list                             # show the bundled agents + skills
mastermind init [--profile <p>]             # scaffold .mastermind/, build index, draft CONTEXT.md (--no-index / --no-claude to skip)
mastermind index .                          # build/refresh the codegraph (incremental; --force to re-parse all)
mastermind watch                            # re-index on file changes
mastermind status                           # file count, symbol count, db path
mastermind doctor                           # environment health check
mastermind serve                            # MCP stdio server (what Claude Code launches)
mastermind setup claude --write-mcp         # register with Claude Code's MCP layer
mastermind verify-spec <path>               # pre-execution gate on a task spec
mastermind audit-spec <path> --since main   # post-execution audit vs a git baseline
mastermind run-task <path>                  # orchestrate verify → execute → audit
mastermind query callers <symbol>           # one-shot CLI query (agents use the MCP tools)
mastermind uninstall [--scope <s>]          # remove setup; --scope global|all for the global MCP entry
```

`setup claude` is safe by default — without `--write-mcp` it prints the diff and exits. Run `mastermind <command> --help` for full options.

## Install options

**Global** (recommended) — puts `mastermind` on PATH; `setup claude --write-mcp` registers `command: "mastermind"` at user scope.

```sh
npm install -g @xcraftmind/mastermind
```

**Project-local** — reproducible and version-pinned with the repo. Writes `./.mcp.json` with `command: "./node_modules/.bin/mastermind"` (commit it; keep ignoring `.mastermind/`).

```sh
npm install -D @xcraftmind/mastermind
npx mastermind setup claude --project . --write-mcp
```

**One-shot** (no install) — fine for one-off commands; avoid for the long-running `serve`.

```sh
npx -y @xcraftmind/mastermind doctor
```

**From source** — for unsupported platforms or contributors. Installs the binary as `mmcg` (same code and subcommands), without the workflow bundle.

```sh
cargo install mmcg          # requires Rust 1.75+
```

## Supported platforms

| OS | Arch |
|---|---|
| macOS | aarch64 (Apple Silicon), x86_64 (Intel) |
| Linux · glibc | x86_64, aarch64 |
| Linux · musl (Alpine) | x86_64, aarch64 |
| Windows | x86_64 |

Other targets fall back to `cargo install mmcg`.

## Packaging

Prebuilt-platform-package pattern (the same one `esbuild`, `swc`, and `turbo` use): the root package is pure JS wrappers and lists the seven `@xcraftmind/mmcg-*` platform binaries as `optionalDependencies` — npm installs only the one matching your host and skips the rest. **No `postinstall` script, no network calls beyond the npm registry.**

## Links

- **Source & docs** — [github.com/xcrft/mastermind](https://github.com/xcrft/mastermind)
- **Changelog** — [CHANGELOG.md](https://github.com/xcrft/mastermind/blob/main/CHANGELOG.md)
- **Rust crate** — [crates.io/crates/mmcg](https://crates.io/crates/mmcg)
- **MCP** — [modelcontextprotocol.io](https://modelcontextprotocol.io)

## License

MIT — see [LICENSE](./LICENSE).
