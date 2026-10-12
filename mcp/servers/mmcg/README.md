---
name: mmcg
description: Mastermind Codegraph — local multi-language code indexer for Python, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust, C#, Go, Java, PHP, and C/C++. Stores symbols, calls, imports, evidence, and project history in SQLite and exposes bounded MCP tools.
metadata:
  version: 3.2.4
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

mmcg indexes repository symbols, calls, imports and project evidence into a
local SQLite database. Use the CLI, MCP server or Lens UI to inspect structure,
change impact and verification evidence.

## Install

With Cargo (Rust 1.96+):

```bash
cargo install mmcg --locked
mmcg --version
```

Or install a prebuilt binary with Node.js 24+:

```bash
npm install -g @xcraftmind/mastermind
mastermind --version
```

The npm package provides both `mastermind` and `mmcg`. Cargo provides `mmcg`.
SQLite and tree-sitter are bundled.

## Index and review

Run inside a Git repository. Replace `main` with the baseline you want to review.

```bash
mmcg index .
mmcg map .
mmcg impact --since main
mmcg ui --since main
```

The default index is `.mastermind/mmcg.db`. Indexing is incremental. Use
`mmcg watch` while editing or `mmcg index . --force` to reparse all files.

| Task | Command |
|---|---|
| Look up a definition | `mmcg query search PaymentService` |
| Find containing callers | `mmcg query callers charge` |
| Find symbols by concept | `mmcg concept "payment retry handler" --top 10` |
| Build a question-specific shortlist | `mmcg repo-map "payment retry handler" --source-sha256 <verified-source-digest> --budget-bytes 1600` |
| Prepare a role briefing | `mmcg brief --role executor --since main --budget-tokens 2000` |
| Search project history | `mmcg history "retry policy"` |
| Add compiler-resolved evidence | `mmcg enrich --scip index.scip` |
| Add declarative facts | `mmcg enrich --facts facts.json` |

Supported languages: Python and type stubs, TypeScript/TSX, JavaScript/JSX,
Vue SFC, Rust, C#, Go, Java, PHP and C/C++.

`repo-map` returns a JSON navigation payload using `focused_shortlist_v3`: weighted
task-term overlap, grouped file locations, at most three files and three definitions
per file. It leaves spare budget unused and excludes declared test functions. It
is an experimental query heuristic, not PageRank or source evidence. Read original
source at the selected locations and follow the relevant call paths.

Before requesting a map, validate the source projection, index freshness and
allowlist, then provide that projection's SHA-256 as `--source-sha256`. The digest
is supplied by the caller; this command does not verify an arbitrary digest against
live files. It opens the index read-only, checks schema compatibility and rejects
candidate sets larger than 20,000 instead of silently ranking a partial prefix.
Invalid digest/budget/location or an index change exits unsuccessfully. Rebuild a
stale index against the intended scope and generate a new binding before retrying.

## Connect an agent

```bash
mmcg setup claude --scope user          # preview
mmcg setup claude --scope user --write  # apply
```

For direct stdio use, run `mmcg serve`. Structural MCP queries may refresh the
managed index before reading it. Custom indexes require an explicit refresh.
Other tools are read-only except local scratchpad writes and task-mining submissions.

See [client integrations](https://github.com/xcrft/mastermind/tree/main/docs/integrations)
for Claude Code, Codex, Cursor, Continue and generic MCP clients.

## Interpret results

| Result | Evidence and limits |
|---|---|
| Default graph | Syntactic, with gaps for name collisions, dynamic dispatch, reflection, generated code and cross-language calls |
| SCIP | Separately identified compiler evidence |
| Imported facts | Source and revision metadata, no executable plugin loading |
| Unreferenced symbols | Candidates for review, no proof of runtime unreachability |
| Candidate tests | Suggested coverage, no observed test result |
| Stale inputs, omissions and work limits | Reported in the result, source-current evidence still needs interpretation |

Indexing, queries and Lens run locally. Explicit model-backed commands such as
native execution, review or persona analysis use the selected client/provider.
See the workflow and persona guides before enabling them.

## Reference and development

- [CLI, MCP tools, limits and precision](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md)
- [Task workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md)
- [Persona capture and review](https://github.com/xcrft/mastermind/blob/main/docs/guides/persona-hooks.md)
- [Fact-ingestion contract](https://github.com/xcrft/mastermind/blob/main/docs/fact-ingestion-sdk.md)
- [Contributing and validation](https://github.com/xcrft/mastermind/blob/main/CONTRIBUTING.md)
- [Benchmark methodology](https://github.com/xcrft/mastermind/blob/main/docs/benchmarks.md)

License: [MIT](https://github.com/xcrft/mastermind/blob/main/LICENSE).
