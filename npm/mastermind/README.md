# @xcraftmind/mastermind

[![npm version 3.0.1](https://img.shields.io/badge/npm-v3.0.1-CB3837?logo=npm)](https://www.npmjs.com/package/@xcraftmind/mastermind)
[![CI status](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg)](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml)
[![MIT license](https://img.shields.io/badge/license-MIT-4f46e5.svg)](https://github.com/xcrft/mastermind/blob/main/LICENSE)

Local repository context and task-evidence workflows for coding agents.
The package supplies a prebuilt binary as `mastermind` and `mmcg`.
It indexes source code, Markdown documentation, and Vue SFC script blocks with
source-backed symbols and relationships.

## Quick start

Requires Node.js 24+. Run inside your repository:

```bash
npm install -g @xcraftmind/mastermind
mastermind init
```

The first interactive run asks for Claude Code, Codex, both, or no client, then
the mining mode. It scaffolds the project, indexes code and documentation, and
configures the selected client. An unattended first run without `--client`
stays local. Choices are saved in `.mastermind/setup.json`.

| Command | Purpose |
|---|---|
| `mastermind status --json` | Inspect component state and the next task action |
| `mastermind update` | Update in the existing npm scope and refresh installed workflows |
| `mastermind update --dry-run` | Preview locally without changes or network calls |
| `mastermind ui --since main` | Open a read-only review against your Git baseline |
| `mastermind --help` | Show onboarding and project commands |
| `mmcg --help` | Show the complete native CLI |

Restart the selected client after setup. Registration does not establish client
trust or a live connection. Local capture is the default for a selected client.
Semantic mining, profile read access, and prompt refinement require separate
opt-ins. Repeated `init` preserves miner budgets.

| Platform | Support |
|---|---|
| macOS arm64 or x64 | Prebuilt native tools, hooks, and managed mining |
| Linux arm64 or x64, glibc or musl | Prebuilt native tools, hooks, and managed mining |
| Windows x64 | Local indexing and client setup with `--mining off` |
| Rust toolchain | Not required for this package |

Native hooks and managed mining are unsupported on Windows. See the
[package manifest](https://github.com/xcrft/mastermind/blob/main/npm/mastermind/package.json)
for binary packages.

## Documentation

| Guide | Covers |
|---|---|
| [Getting started](https://github.com/xcrft/mastermind/blob/main/docs/getting-started.md) | Mining modes, budgets, permissions, updates, and alternative installations |
| [Architecture](https://github.com/xcrft/mastermind/blob/main/docs/architecture.md) | Context layers and evidence boundaries |
| [Task workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md) | Scope, checks, review, and completion |
| [CLI and MCP reference](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md) | Native commands, tools, and language coverage |
| [Client integrations](https://github.com/xcrft/mastermind/tree/main/docs/integrations) | Advanced setup, scope, and removal |

Cargo package: [mmcg](https://crates.io/crates/mmcg).
License: [MIT](https://github.com/xcrft/mastermind/blob/main/LICENSE).
