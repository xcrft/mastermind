# Mastermind

<p align="center">
  <img src="docs/assets/banner.webp" alt="Mastermind — a local codegraph and verifiable workflow for AI coding agents" width="720">
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@xcraftmind/mastermind"><img src="https://img.shields.io/npm/v/@xcraftmind/mastermind.svg" alt="npm version"></a>
  <a href="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml"><img src="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="evals/benchmarks.md"><img src="https://img.shields.io/badge/evals-critic%20%2B%20auditor%20%2B%20intake%20%2B%20workflow-yellowgreen" alt="Evals"></a>
</p>

**A local codegraph and verifiable workflow for AI coding agents.**

Mastermind gives Claude Code, Codex, Cursor, and Continue a structural view of your code: what exists, who calls it, what a change can affect, and which tests are relevant. Its optional workflow checks an agent's plan and implementation against the real repository instead of trusting the agent's memory.

## What you get

| When you need to… | Mastermind gives you… |
|---|---|
| Understand an unfamiliar repository | Components, entry points, dependencies, hotspots, and cycles |
| Change code safely | Changed symbols, affected callers, API crossings, and blast radius |
| Choose focused tests | Direct, transitive, and heuristic test candidates with evidence |
| Verify agent work | Pre-execution spec checks, post-execution diff audits, and signed evidence |

The codegraph stays on your machine in a local SQLite database. No source code is uploaded by Mastermind.

## Try it in two minutes

Requires Node.js 24+. Prebuilt binaries are included; Rust is not required.

### 1. Install once

```bash
npm install -g @xcraftmind/mastermind
```

Connect the client you use:

```bash
mastermind install --client all                # Claude + Codex workflow adapters and MCP
mastermind setup cursor --scope user --write   # Cursor MCP
mastermind setup continue --scope user --write # Continue MCP
```

Use `mastermind install` for Claude only or `mastermind install --client codex`
for Codex only. `mastermind doctor --workflow --client all` verifies that the
ownership manifests, artifact lists, and SHA-256 content match the current package.

Global installation and MCP registration do **not** require a project or `mastermind init`. Setup is dry-run-first when `--write` is omitted.

### 2. Use it in a repository

```bash
cd your-project
mastermind index .
mastermind map .
mastermind impact --since main
```

`index` is enough for codegraph, map, and impact features. Run `mastermind init` only when you want the complete spec-driven project workflow:

```bash
mastermind init
mastermind doctor
```

See [Getting started](docs/getting-started.md) for global vs per-repository state, project-local installation, and client-specific setup.

## Core workflows

### Map a project

```bash
mastermind map .                     # readable architecture briefing
mastermind map src --format mermaid  # scoped diagram
mastermind map . --format json       # stable schema for automation
mastermind map . --production-only   # hide tests, fixtures, examples, generated/vendor code
```

The map highlights languages, components, entry points, dependency boundaries, hotspots, and cycles without asking an agent to grep the entire repository.

### Understand change and test impact

```bash
mastermind impact --since main
mastermind impact --since HEAD~1 --format json
```

Impact analysis compares a Git baseline with committed, staged, unstaged, and untracked work. It reports symbol-level changes, affected callers, component crossings, and candidate tests. Focused candidates are evidence for prioritization, not a replacement for the repository's required test suite.

### Query the graph from an agent

The MCP server exposes 23 bounded tools for symbol search, callers/callees, imports, architecture maps, change impact, test impact, cycles, API surface, and task history. The same engine is available through the CLI.

```text
"Does parseConfig exist?"
"Who calls createSession?"
"What could this branch affect?"
"Which tests are connected to these changes?"
```

See the [mmcg reference](docs/reference/mmcg.md) for the complete tool and protocol contract.

### Plan, execute, and verify

The optional workflow turns an implementation request into a checked sequence:

```mermaid
flowchart LR
  U["Request"] --> P["Plan from the live codegraph"]
  P --> V["Verify the spec"]
  V --> E["Implement"]
  E --> A["Audit the real diff"]
  A --> R["Release evidence"]
```

The planner, executor, critic, and auditor have separate responsibilities. Deterministic Rust gates verify file, symbol, snapshot, scope, and test claims before and after implementation. Read [How the workflow works](docs/workflow.md) for the roles and task lifecycle.

### Produce verifiable audit evidence

Mastermind can seal audit results with SHA-256 integrity and detached Ed25519 signatures. The included GitHub Action verifies repository, baseline, head, worktree, signature, and policy inputs before evidence is published.

See [Verifiable audits and GitHub Action](docs/github-action.md).

## What is global and what is per project?

| State | Scope | Created by |
|---|---|---|
| `mastermind` CLI | Global or project-local npm install | `npm install` |
| Claude workflow agents and skills | Global | `mastermind install` |
| Codex workflow skills | Global | `mastermind install --client codex` |
| MCP client registration | User or project, depending on client | `mastermind setup …` |
| `.mastermind/mmcg.db` codegraph | Per repository | `mastermind index .` or `mastermind init` |
| Task specs and project context | Per repository, optional | `mastermind init` |

## Support

- **Clients:** Claude Code, Codex, Cursor, Continue, and generic MCP stdio clients
- **Languages:** Python, TypeScript/TSX, JavaScript/JSX, Rust, C#, Go, Java, PHP, and C/C++
- **Platforms:** macOS arm64/x64, Linux glibc and musl arm64/x64, Windows x64
- **Privacy:** local parsing and local SQLite storage; no daemon and no postinstall download

The graph is syntactic rather than compiler-semantic. Dynamic dispatch, reflection, re-exports, overload resolution, and cross-language calls can reduce precision. Mastermind reports bounded results and precision notes instead of presenting incomplete analysis as certain.

## Documentation

- [Getting started](docs/getting-started.md)
- [Claude Code](docs/integrations/claude-code.md) · [Codex](docs/integrations/codex.md) · [Cursor](docs/integrations/cursor.md) · [Continue](docs/integrations/continue.md) · [Generic MCP](docs/integrations/generic-mcp.md)
- [Workflow](docs/workflow.md)
- [mmcg technical reference](docs/reference/mmcg.md)
- [GitHub Action and audit security model](docs/github-action.md)
- [Changelog](CHANGELOG.md)

## Build from source

Rust 1.96+ is required for source builds:

```bash
cargo install mmcg
# or from a clone
cargo install --path mcp/servers/mmcg
```

The cargo-installed command is `mmcg`; the npm package exposes the same binary as both `mastermind` and `mmcg`.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for the project layout, checks, evals, and pull-request conventions.

## License

MIT — see [LICENSE](LICENSE).
