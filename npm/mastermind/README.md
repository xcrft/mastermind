# @xcraftmind/mastermind

[![npm version 4.0.1](https://img.shields.io/badge/npm-v4.0.1-CB3837?logo=npm)](https://www.npmjs.com/package/@xcraftmind/mastermind)
[![CI status](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg)](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml)
[![MIT license](https://img.shields.io/badge/license-MIT-4f46e5.svg)](https://github.com/xcrft/mastermind/blob/main/LICENSE)

Mastermind indexes your code and documentation locally, gives coding agents
that context over MCP and prompt hooks, and checks a task's evidence before it
counts as done. This package provides the prebuilt native binary as both
`mastermind` and `mmcg`. It indexes source code, Markdown and Vue SFC script
blocks.

## Quick start

You need Node.js 24 or later, and no Rust toolchain. Run inside your repository:

```bash
npm install -g @xcraftmind/mastermind
mastermind init
```

`init` sets up the clients it finds, indexes the repository and saves its
choices in `.mastermind/setup.json`. Restart the client afterwards and review
the hook definitions in `/hooks`.
[Getting started](https://github.com/xcrft/mastermind/blob/main/docs/getting-started.md)
covers the options and mining modes.

| Command | Purpose |
|---|---|
| `mastermind status --json` | Component state and the next task action |
| `mastermind update` | Updates in the existing npm scope and refreshes the installed workflows |
| `mastermind update --dry-run` | Local preview, with no changes and no network calls |
| `mastermind update --move-aside` | Moves edited, linked or unowned workflow entries into `<client root>/.mastermind-preserved/`, then installs the bundled copies |
| `mastermind ui --since main` | Opens the read-only review UI against a Git baseline |
| `mastermind --help` | Setup and project commands |
| `mmcg --help` | The complete native CLI |

| Platform | Support |
|---|---|
| macOS arm64 or x64 | Native tools, hooks and managed mining |
| Linux arm64 or x64, glibc or musl | Native tools, hooks and managed mining |
| Windows x64 | Local indexing and client setup with `--mining off`. No native hooks or managed mining |

The binary packages are listed in the
[package manifest](https://github.com/xcrft/mastermind/blob/main/npm/mastermind/package.json).

## Documentation

| Guide | Covers |
|---|---|
| [Getting started](https://github.com/xcrft/mastermind/blob/main/docs/getting-started.md) | Setup, mining modes, updates and other installation methods |
| [Persona hooks](https://github.com/xcrft/mastermind/blob/main/docs/guides/persona-hooks.md) | Capture, candidate review and the optional separate miner |
| [Architecture](https://github.com/xcrft/mastermind/blob/main/docs/architecture.md) | Context layers and evidence boundaries |
| [Workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md) | Scope, checks, review and completion |
| [CLI and index](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md) | Native commands and language coverage |
| [MCP tools](https://github.com/xcrft/mastermind/blob/main/docs/reference/mcp.md) | The MCP server and its tools |
| [Client integrations](https://github.com/xcrft/mastermind/tree/main/docs/integrations) | Scopes, other MCP clients and removal |

Cargo package: [mmcg](https://crates.io/crates/mmcg).
License: [MIT](https://github.com/xcrft/mastermind/blob/main/LICENSE).
