# @xcraftmind/mastermind

[![npm](https://img.shields.io/badge/npm-v1.2.1-CB3837?logo=npm)](https://www.npmjs.com/package/@xcraftmind/mastermind)
[![CI](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg)](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/xcrft/mastermind/blob/main/LICENSE)

**A local codegraph and verifiable workflow for AI coding agents.**

Mastermind gives Claude Code, Codex, Cursor, and Continue structural project
maps, change and test impact, MCP code queries, and diff-backed implementation
audits. Deterministic indexing and style mining stay local. Explicit
agent-assisted modes disclose when they invoke the configured AI client with
repository content or bounded samples.

## Install

Requires Node.js 24+. Prebuilt binaries are included for macOS, Linux, and
Windows.

```bash
npm install -g @xcraftmind/mastermind
```

Connect your clients once:

```bash
mastermind install --client all
mastermind setup cursor --scope user --write
mastermind setup continue --scope user --write
```

Global installation and client setup do not require a project or
`mastermind init`.

## Use it in a repository

```bash
cd your-project
mastermind index .
mastermind map .
mastermind impact --since main
mastermind ui --since main
mastermind miner profile .  # optional personal profile; no init required
```

Run `mastermind init` only when you want the complete spec-driven workflow.
Small changes can use the codegraph directly without creating a task spec.

## What it provides

- Architecture maps plus runtime, state, retry, and compatibility reviews
- Symbol search, callers, imports, cycles, and API surface
- Changed symbols, affected callers, component crossings, and candidate tests
- A local, read-only diff-first Lens UI backed by the same map and impact engine
- Direct, verified, and strict task workflows based on change risk
- Executor reports checked against the real Git diff and codegraph
- SHA-256 and Ed25519 audit evidence with a pinned GitHub Action
- An optional user-global style portrait used only as advisory planner/executor input

Supported languages: Python, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust,
C#, Go, Java, PHP, and C/C++.

## Documentation

[Getting started](https://github.com/xcrft/mastermind/blob/main/docs/getting-started.md) ·
[Client setup](https://github.com/xcrft/mastermind/tree/main/docs/integrations) ·
[Workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md) ·
[Technical reference](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md)

MIT — [source and license](https://github.com/xcrft/mastermind)
