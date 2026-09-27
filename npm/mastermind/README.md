# @xcraftmind/mastermind

[![npm version 2.1.1](https://img.shields.io/badge/npm-v2.1.1-CB3837?logo=npm)](https://www.npmjs.com/package/@xcraftmind/mastermind)
[![CI status](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg)](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml)
[![MIT license](https://img.shields.io/badge/license-MIT-4f46e5.svg)](https://github.com/xcrft/mastermind/blob/main/LICENSE)

Local repository context and task-evidence workflows for coding agents.
The package supplies a prebuilt binary as `mastermind` and `mmcg`.

## Install and review a change

| Requirement | Support |
|---|---|
| Node.js | 24+ |
| macOS | arm64, x64 |
| Linux | glibc or musl, arm64 or x64 |
| Windows | x64 |
| Rust toolchain | Not required |

Source: [package manifest](https://github.com/xcrft/mastermind/blob/main/npm/mastermind/package.json).

```bash
npm install -g @xcraftmind/mastermind
cd your-repository
mastermind index .
mastermind impact --since main
mastermind ui --since main
```

Replace `main` with the baseline. The index lives in `.mastermind/mmcg.db`.
Keep `.mastermind/` out of Git. Lens serves the review on loopback.

| Need | Command |
|---|---|
| Repository structure | `mastermind map .` |
| Role briefing | `mastermind brief --role executor --since main --budget-tokens 2000` |
| Symbol discovery | `mastermind concept "payment retry handler" --top 10` |
| Offline review | `mastermind review export --since main --out mastermind-review` |

The static graph supports Python, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust,
C#, Go, Java, PHP, and C/C++. See
[language coverage](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md#language-coverage)
for extraction limits. SCIP and external facts retain separate provenance.

## Connect a coding client

| Client | Setup |
|---|---|
| Claude Code | `mastermind install --client claude` |
| Codex | `mastermind install --client codex` |
| Both | `mastermind install --client all` |
| Cursor or Continue | `mastermind setup cursor --scope user` or `setup continue --scope user`, then add `--write` |
| Other MCP client | [Generic MCP guide](https://github.com/xcrft/mastermind/blob/main/docs/integrations/generic-mcp.md) |

```bash
mastermind doctor --workflow --client all
```

`core` is the default skill profile. Updates retain the installed profile.
See [client setup](https://github.com/xcrft/mastermind/tree/main/docs/integrations)
for scope and removal.

| Boundary | Behavior |
|---|---|
| Indexing, deterministic queries, Lens | Local |
| Managed graph queries | May refresh the derived index |
| Scratchpad append | Additive local write |
| Personal profile | Separate project/client read grant |
| Native execution, review, AI mining | Explicit operations that may send context to the selected provider |

## Documentation

- [Getting started](https://github.com/xcrft/mastermind/blob/main/docs/getting-started.md)
- [Architecture](https://github.com/xcrft/mastermind/blob/main/docs/architecture.md)
- [Task workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md)
- [CLI and MCP reference](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md)
- [Fact ingestion](https://github.com/xcrft/mastermind/blob/main/docs/fact-ingestion-sdk.md)
- [Benchmarks](https://github.com/xcrft/mastermind/blob/main/docs/benchmarks.md)

Cargo package: [mmcg](https://crates.io/crates/mmcg).
License: [MIT](https://github.com/xcrft/mastermind/blob/main/LICENSE).
