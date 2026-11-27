# @xcraftmind/mastermind

[![npm version 5.0.0](https://img.shields.io/badge/npm-v5.0.0-CB3837?logo=npm)](https://www.npmjs.com/package/@xcraftmind/mastermind)
[![CI status](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg)](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml)
[![MIT license](https://img.shields.io/badge/license-MIT-4f46e5.svg)](https://github.com/xcrft/mastermind/blob/main/LICENSE)

Mastermind indexes your code and documentation locally, gives coding agents
that context over MCP and prompt hooks, and checks a task's evidence before the
task counts as done. This package ships the prebuilt native binary as both
`mastermind` and `mmcg`, plus the workflow skills for Claude Code and Codex. It
indexes source code, Markdown and Vue SFC script blocks.

## Quick start

You need Node.js 24 or later, and no Rust toolchain. Run this inside your
repository:

```bash
npm install -g @xcraftmind/mastermind
mastermind init
mastermind status --json
```

`init` sets up the clients it finds, indexes the repository and saves its
choices in `.mastermind/setup.json`. Restart the client afterwards and review
the hook definitions in `/hooks`.

| Command | Purpose |
|---|---|
| `mastermind update` | Updates in the existing npm scope and refreshes the installed workflows. `--dry-run` previews with no network calls |
| `mastermind ui --since main` | Opens the read-only review UI against a Git baseline |
| `mastermind --help` | Setup and project commands |
| `mmcg --help` | The complete native CLI |

| Platform | Support |
|---|---|
| macOS arm64 or x64 | Native tools, hooks and managed mining |
| Linux arm64 or x64, glibc or musl | Native tools, hooks and managed mining |
| Windows x64 | Local indexing and client setup with `--mining off`. No native hooks or managed mining |

The platform binaries are separate packages, listed in the
[package manifest](https://github.com/xcrft/mastermind/blob/main/npm/mastermind/package.json).

## Documentation

| Guide | Covers |
|---|---|
| [Getting started](https://github.com/xcrft/mastermind/blob/main/docs/getting-started.md) | Setup, clients, mining modes, updates and removal |
| [Workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md) | Scope, checks, review and completion |
| [Persona hooks](https://github.com/xcrft/mastermind/blob/main/docs/guides/persona-hooks.md) | Capture, candidate review and the optional separate miner |
| [Other MCP clients](https://github.com/xcrft/mastermind/blob/main/docs/integrations/generic-mcp.md) | Cursor, Continue and any stdio MCP client |
| [CLI and index](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md) | Native commands and language coverage |
| [MCP tools](https://github.com/xcrft/mastermind/blob/main/docs/reference/mcp.md) | The MCP server and its tools |

Cargo package: [mmcg](https://crates.io/crates/mmcg).
License: [MIT](https://github.com/xcrft/mastermind/blob/main/LICENSE).
