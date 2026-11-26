---
name: mmcg
description: Mastermind Codegraph, a local multi-language code indexer for Python, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust, C#, Go, Java, PHP, and C/C++. Stores symbols, calls, imports, evidence, and project history in SQLite and exposes bounded MCP tools.
metadata:
  version: 4.0.1
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

# mmcg (Mastermind Codegraph)

mmcg indexes repository symbols, calls, imports and project evidence into a
local SQLite database. The CLI, the MCP server and the Lens UI read it to show
structure, change impact and verification evidence. It supports Python and
type stubs, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust, C#, Go, Java, PHP
and C/C++.

## Install

With Cargo and Rust 1.96 or later:

```bash
cargo install mmcg --locked
mmcg --version
```

Cargo installs only `mmcg`. SQLite and tree-sitter are bundled. For the
`mastermind` command and the workflow skills, install the npm package with
Node.js 24 or later instead: `npm install -g @xcraftmind/mastermind`.

## Index and review

Run this inside a Git repository, with `main` replaced by the baseline you want
to review:

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
| Find callers | `mmcg query callers charge` |
| Find symbols by plain-language terms, without a model | `mmcg concept "payment retry handler" --top 10` |
| Check which candidates implement an operation | `mmcg concept-review prepare "payment retry handler" --top 5`, then `mmcg concept-review finish --packet packet.json --assessment assessment.json` |
| Build a question-specific shortlist | `mmcg repo-map "payment retry handler" --source-sha256 <verified-source-digest> --budget-bytes 1600` |
| Prepare a role briefing | `mmcg brief --role executor --since main --budget-tokens 2000` |
| Search project history | `mmcg history "retry policy"` |
| Add compiler-resolved evidence | `mmcg enrich --scip index.scip` |
| Add declarative facts | `mmcg enrich --facts facts.json` |

## Connect an agent

```bash
mmcg setup claude --scope user          # preview
mmcg setup claude --scope user --write  # apply
```

For direct stdio use, run `mmcg serve`. Structural MCP queries may refresh the
managed index before they read it, and a custom index needs an explicit
refresh. The only other write is a task-mining submission. Other clients are
covered in
[getting started](https://github.com/xcrft/mastermind/blob/main/docs/getting-started.md#register-mcp-without-init)
and [other MCP clients](https://github.com/xcrft/mastermind/blob/main/docs/integrations/generic-mcp.md).

## Read the results

| Result | Limits |
|---|---|
| Default graph | Syntactic, with gaps for name collisions, dynamic dispatch, reflection, generated code and cross-language calls |
| SCIP | Compiler evidence, labeled separately |
| Imported facts | Source and revision metadata, with no plugin code loaded |
| Unreferenced symbols | Candidates for review. Code can still run through paths the graph misses |
| Candidate tests | Suggested coverage. No test has run |
| Stale inputs, omissions and work limits | Reported in the result. `query`, `map`, `repo-map` and `impact` print a stale-index warning on stderr |

Indexing, queries and Lens run locally with no model calls. Native execution,
review and persona analysis use the client or provider you select.

## Reference

- [CLI, index, limits and precision](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md)
- [MCP tools](https://github.com/xcrft/mastermind/blob/main/docs/reference/mcp.md)
- [Concept search](https://github.com/xcrft/mastermind/blob/main/docs/reference/concept-search.md)
- [Task workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md)
- [Fact ingestion](https://github.com/xcrft/mastermind/blob/main/docs/fact-ingestion-sdk.md)
- [Contributing](https://github.com/xcrft/mastermind/blob/main/CONTRIBUTING.md)

License: [MIT](https://github.com/xcrft/mastermind/blob/main/LICENSE).
