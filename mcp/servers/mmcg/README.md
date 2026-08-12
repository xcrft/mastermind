---
name: mmcg
description: Mastermind Codegraph — fast multi-language code indexer (Python + TypeScript/TSX + JavaScript/JSX + Vue SFC + Rust + C# + Go + Java + PHP + C/C++) exposed over MCP. Indexes symbols, calls, imports, and durable project history into a local SQLite database and exposes 24 bounded tools for AI coding agents.
metadata:
  version: 1.2.1
  authors:
    - mastermind
  tags:
    - mmcg
    - codegraph
    - python
    - typescript
    - javascript
    - vue
    - rust
    - csharp
    - go
    - java
    - php
    - cpp
  transport: stdio
  source: this repository
---

# mmcg — Mastermind Codegraph

mmcg is the local structural engine inside [Mastermind](https://github.com/xcrft/mastermind). It indexes symbols, calls, and imports from a repository into SQLite, then exposes the graph through a CLI and MCP server.

Use it to answer questions such as:

- Does this symbol exist and where is it defined?
- Who calls it, and what is its transitive blast radius?
- What are the components, entry points, hotspots, and cycles?
- What can the current worktree change affect?
- Which tests are structurally connected to that change?

The npm package exposes this binary as both `mastermind` and `mmcg`. Cargo installation uses the `mmcg` command.

## Install

Prebuilt binaries, Node.js 24+:

```bash
npm install -g @xcraftmind/mastermind
mastermind --version
```

Build from source, Rust 1.96+:

```bash
cargo install mmcg
mmcg --version
```

SQLite and tree-sitter are bundled. No system SQLite or parser libraries are required.

## Quick start

```bash
cd your-project
mmcg index .
mmcg status
mmcg map .
mmcg impact --since main
mmcg ui --since main
```

`index` is incremental by default and writes `.mastermind/mmcg.db`. Discovery
honors Git and `.ignore` rules while retaining tracked sources, matches
extensions case-insensitively, parses `.pyi` and BOM-marked UTF-16 sources, and
reports bounded path samples for skipped inputs. It automatically rebuilds when
stored extractor semantics are incompatible. Use `mmcg watch` to keep it
refreshed while editing.

Run the stdio MCP server directly:

```bash
mmcg serve
```

When installed through npm, prefer the dry-run-first client setup commands:

```bash
mastermind setup claude --scope user          # preview
mastermind setup claude --scope user --write  # apply
```

See the [client integration guides](https://github.com/xcrft/mastermind/tree/main/docs/integrations) for Claude Code, Codex, Cursor, Continue, and generic MCP clients.

## Capabilities

| Area | Examples |
|---|---|
| Symbol graph | Search, callers, callees, imports, outlines, API surface |
| Architecture | Project map, centrality, dependency cycles, unreferenced candidates |
| Change analysis | Git-aware symbol changes, blast radius, component crossings |
| Local review UI | Read-only diff-first Lens with bounded SARIF, coverage, ownership, and churn overlays |
| Test selection | Direct, transitive, and heuristic candidates with evidence |
| Workflow gates | `verify-spec`, `audit-spec`, and `run-task` |
| Local coordination | Bounded additive scratchpad and indexed project history |

The MCP surface contains 23 read-only tools and one additive local scratchpad
write. Results are bounded and return precision or truncation notes when the
engine cannot prove completeness.

## Supported languages

- Python and Python type stubs
- TypeScript and TSX
- JavaScript and JSX
- Vue SFC
- Rust
- C#
- Go
- Java
- PHP
- C and C++

## Precision model

mmcg is a syntactic graph, not a compiler or language server:

- Call resolution is primarily name-based rather than type-based.
- Dynamic dispatch, reflection, generated code, and cross-language calls may be invisible.
- Import paths reflect source spelling and do not resolve every re-export.
- C/C++ include paths resolve to indexed files for architecture maps and cycle
  detection, but included contents and compiler semantics are not expanded.
- Dead-code and test-impact results are candidates to review, not deletion or test-skipping authorization.

These trade-offs keep the index local, fast, portable, and easy for agents to query. The engine fails closed on stale index, root, Git snapshot, or work-limit conditions in change-impact workflows.

## Reference

The exhaustive technical documentation is intentionally separate from this crate landing page:

- [CLI, indexing, MCP protocol, all tools, and limitations](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md)
- [Getting started](https://github.com/xcrft/mastermind/blob/main/docs/getting-started.md)
- [Mastermind workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md)
- [Verifiable audits and GitHub Action](https://github.com/xcrft/mastermind/blob/main/docs/github-action.md)

## Development

```bash
cargo test --all --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo fmt --all -- --check
cargo bench --bench indexer
```

The index benchmark emits schema-v1 JSON for cold, warm, and 10%-incremental
runs plus peak process RSS. Defaults are 1,000 synthetic Rust files with 20
symbols each. Override them with `MMCG_BENCH_FILES`,
`MMCG_BENCH_SYMBOLS_PER_FILE`, and `MMCG_BENCH_CHANGED_FILES`. Treat results as
same-machine regression evidence, not a portable performance promise.

See [CONTRIBUTING.md](https://github.com/xcrft/mastermind/blob/main/CONTRIBUTING.md) for the repository-wide contribution guide.

## License

MIT — see [LICENSE](https://github.com/xcrft/mastermind/blob/main/LICENSE).
