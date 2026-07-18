# @xcraftmind/mastermind

[![npm](https://img.shields.io/npm/v/@xcraftmind/mastermind.svg)](https://www.npmjs.com/package/@xcraftmind/mastermind)
[![downloads](https://img.shields.io/npm/dm/@xcraftmind/mastermind.svg)](https://www.npmjs.com/package/@xcraftmind/mastermind)
[![node](https://img.shields.io/node/v/@xcraftmind/mastermind.svg)](https://nodejs.org)
[![license: MIT](https://img.shields.io/npm/l/@xcraftmind/mastermind.svg)](https://github.com/xcrft/mastermind/blob/main/LICENSE)

**A local codegraph and verifiable workflow for AI coding agents.**

Mastermind helps Claude Code, Codex, Cursor, and Continue understand repository structure, estimate change and test impact, and verify implementation claims against the real diff. The npm package ships prebuilt native binaries; no Rust toolchain or postinstall download is required.

**Languages:** Python, TypeScript/TSX, JavaScript/JSX, Rust, C#, Go, Java, PHP, and C/C++.

## Install once

Requires Node.js 24+.

```bash
npm install -g @xcraftmind/mastermind
```

Connect the client you use:

```bash
mastermind install                              # Claude workflow + skills + MCP
mastermind setup codex --scope user --write    # Codex MCP
mastermind setup cursor --scope user --write   # Cursor MCP
mastermind setup continue --scope user --write # Continue MCP
```

These commands do not require a project or `mastermind init`. Omit `--write` to preview a redacted setup plan.

## Use it in a repository

Indexing alone enables codegraph, map, and impact features:

```bash
cd your-project
mastermind index .
mastermind map .
mastermind impact --since main
```

Enable the complete spec-driven workflow only when you need it:

```bash
mastermind init
mastermind doctor
```

The index is stored per repository in `.mastermind/mmcg.db` and stays local.

## Project-local installation

Pin Mastermind to a repository:

```bash
npm install -D @xcraftmind/mastermind
npx mastermind setup claude --scope project --root . --write
```

Codex supports user scope only. Claude and Cursor support user and project scopes; Continue uses a Mastermind-owned YAML file. Generic MCP clients require an explicit config path.

## Included workflows

- Architecture maps and structural MCP queries
- Change impact and evidence-ranked test candidates
- Plan → execute → audit task contracts
- Cross-client MCP setup
- SHA-256 and Ed25519 audit attestations
- GitHub Action integration

Run `mastermind list` to inspect the installed Claude workflow bundle and `mastermind <command> --help` for CLI options.

## Platforms

Prebuilt packages are published for macOS arm64/x64, Linux glibc and musl arm64/x64, and Windows x64. Other targets can build `mmcg` from source with Rust 1.96+.

## Documentation

- [Product overview](https://github.com/xcrft/mastermind)
- [Getting started](https://github.com/xcrft/mastermind/blob/main/docs/getting-started.md)
- [Client integrations](https://github.com/xcrft/mastermind/tree/main/docs/integrations)
- [Workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md)
- [mmcg reference](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md)
- [Changelog](https://github.com/xcrft/mastermind/blob/main/CHANGELOG.md)

## License

MIT — see [LICENSE](https://github.com/xcrft/mastermind/blob/main/LICENSE).
