---
name: mmcg
description: Mastermind Codegraph — local multi-language code indexer for Python, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust, C#, Go, Java, PHP, and C/C++. Stores symbols, calls, imports, evidence, and project history in SQLite and exposes 28 bounded MCP tools.
metadata:
  version: 2.0.1
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

Ask architecture questions against the repository on your machine, not against
a pasted fragment or a remote black box.

mmcg is the local structural engine inside
[Mastermind](https://github.com/xcrft/mastermind). It indexes symbols, calls,
imports, and evidence into SQLite, then exposes the same state through CLI,
Lens, and MCP.

Use it to answer questions such as:

- Does this symbol exist and where is it defined?
- Who calls it, and what is its transitive blast radius?
- What are the components, entry points, hotspots, and cycles?
- What can the current worktree change affect?
- Which tests are structurally connected to that change?

The npm package exposes this binary as both `mastermind` and `mmcg`. Cargo
installation uses `mmcg`.

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

## See your first result

```bash
cd your-project
mmcg index .
mmcg enrich --scip index.scip # optional compiler-resolved overlay
mmcg enrich --facts facts.json # optional declarative fact overlay
mmcg status
mmcg map .
mmcg impact --since main
mmcg temporal --since main
mmcg ui --since main
```

`index` is incremental by default and writes `.mastermind/mmcg.db`. Discovery
honors Git and `.ignore` rules while retaining tracked sources, matches
extensions case-insensitively, parses `.pyi` and BOM-marked UTF-16 sources, and
reports bounded path samples for skipped inputs. It automatically rebuilds when
stored extractor semantics are incompatible. Use `mmcg watch` to keep it
refreshed while editing.

Structural MCP queries refresh the managed `.mastermind/mmcg.db` on demand
before querying. The canonical database path, not mutable SQLite metadata,
selects the repository root. Custom external indexes remain manual-refresh-only;
failed or unavailable refreshes return a structured `index_stale` result.

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

## What one graph can answer

| Area | Examples |
|---|---|
| Symbol graph | Search, callers, callees, imports, outlines, API surface |
| Semantic overlay | Optional SCIP definitions, references, implementations, and provenance |
| Extension facts | Revision-bound declarative annotations and relationships with no plugin execution |
| Architecture | Project map, temporal drift, centrality, dependency cycles, unreferenced candidates |
| Change analysis | Git-aware symbol changes, blast radius, component crossings |
| Review evidence | Read-only diff-first Lens plus autonomous HTML/SARIF/summary packages with revision and evidence digests |
| Test selection | Direct, transitive, and heuristic candidates with evidence |
| Workflow gates | `verify-spec`, `audit-spec`, and `run-task` |
| Local coordination | Bounded additive scratchpad and indexed project history |

The MCP surface contains 19 non-destructive queries that may refresh the managed
derived index, 8 read-only tools, and one additive local scratchpad write.
Results are bounded and return precision or truncation notes when the engine
cannot prove completeness.

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

## Know the evidence boundary

The default mmcg graph is syntactic, not a compiler or language server:

- Call resolution is primarily name-based rather than type-based.
- Dynamic dispatch, reflection, generated code, and cross-language calls may be invisible.
- Import paths reflect source spelling and do not resolve every re-export.
- C/C++ include paths resolve to indexed files for architecture maps and cycle
  detection, but included contents and compiler semantics are not expanded.
- Dead-code and test-impact results are candidates to review, not deletion or test-skipping authorization.

This model keeps the default index local and independent of language
toolchains. Change-impact workflows fail closed on stale index, root, Git
snapshot, and work-limit conditions.

`mmcg enrich --scip index.scip` is an optional second layer. It stores
compiler-resolved facts separately, exposes them through `query semantic` and
`mmcg_semantic`, and lets Lens prefer exact SCIP evidence while retaining the
Tree-sitter topology and no-toolchain fallback. Embedded SCIP document text is
verified when available; `project_root` must identify this repository unless
every document embeds matching text. Later source changes suppress stale
semantic facts.

`mmcg enrich --facts facts.json` is the safe community-extension boundary. A
strict [`mastermind-facts/v1`](https://github.com/xcrft/mastermind/blob/main/docs/fact-ingestion-sdk.md)
manifest declares capabilities and binds every source/provenance artifact to
the exact repository identity, Git revision, byte size, and SHA-256 digest.
Mastermind validates the whole manifest before atomically replacing one
producer dataset in private normalized tables. `query facts`, the fixed
read-only `mmcg_facts` tool, and Lens can read it; producers cannot load native
code, register MCP handlers or policy rules, change graph topology, or access
SQLite directly.

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

The benchmark emits schema-v1 JSON for cold, warm, and 10%-incremental runs
plus peak process RSS. See the
[methodology and reference results](https://github.com/xcrft/mastermind/blob/main/docs/benchmarks.md).
Treat results as same-machine regression evidence, not a portable performance
promise.

See [CONTRIBUTING.md](https://github.com/xcrft/mastermind/blob/main/CONTRIBUTING.md) for the repository-wide contribution guide.

## License

MIT — see [LICENSE](https://github.com/xcrft/mastermind/blob/main/LICENSE).
