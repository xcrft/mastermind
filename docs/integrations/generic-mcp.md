# Generic MCP client integration

`mastermind serve` is a standard [MCP](https://modelcontextprotocol.io) stdio server. Any MCP-capable client can connect to it.

## Server spec

| property | value |
|---|---|
| transport | stdio |
| command | `mastermind serve` |
| protocol | MCP 2025-11-25; legacy 2024-11-05 |
| tools | 23: 22 read-only, 1 additive local write (see below) |
| resources | none |
| prompts | none |

Current clients receive behavior annotations and structured results after the initialized notification; legacy clients retain content-only results. Input frames are limited to 1 MiB and serialized result payloads to 8 MiB.

## Safe setup

```text
mastermind setup <claude|cursor|codex|continue|generic> \
  --scope <project|user> [--root .] [--config PATH] [--write] [--remove] [--force]
```

Generic setup always requires an explicit JSON path:

```bash
mastermind setup generic --scope project --config ./mcp.json        # dry-run
mastermind setup generic --scope project --config ./mcp.json --write
```

The command preserves unrelated root fields and MCP servers. Customized replacement or removal requires `--force`; `--force` never implies `--write`, and old bytes are backed up privately under `~/.mastermind/setup-backups/` before a forced mutation. `mastermind doctor` treats configuration as bounded data and never executes a configured command.

## Config snippet

The standard MCP client config format:

```json
{
  "mcpServers": {
    "mmcg": {
      "command": "mastermind",
      "args": ["serve"]
    }
  }
}
```

To target a specific index file (useful when running multiple projects):

```json
{
  "mcpServers": {
    "mmcg": {
      "command": "mastermind",
      "args": ["--index", "/path/to/.mastermind/mmcg.db", "serve"]
    }
  }
}
```

The `--index` flag is global and must come before `serve`.

## Available tools

| tool | description |
|---|---|
| `mmcg_search` | Find symbols by name (exact or prefix) |
| `mmcg_callers` | List callers of a symbol |
| `mmcg_callees` | List callees of a symbol |
| `mmcg_impact` | Transitive blast radius of a symbol change |
| `mmcg_imports` | Imports of a file |
| `mmcg_imported_by` | Files that import a given path or name |
| `mmcg_symbols_in_file` | All symbols defined in a file |
| `mmcg_outline` | Symbol tree of a file (classes own methods) |
| `mmcg_files` | List indexed files, optionally filtered by prefix or language |
| `mmcg_api_surface` | Symbols in a path prefix referenced from outside it |
| `mmcg_unreferenced` | Dead-code candidates (symbols nothing calls) |
| `mmcg_dependency_cycles` | Circular imports in the file-level graph |
| `mmcg_symbols_changed_since` | Symbol-level diff between a git ref and current index |
| `mmcg_centrality` | Most-referenced symbols by in-degree |
| `mmcg_map` | Schema-v1 project map with lexical scope (`%`/`_` literal), directory-relative components, `depth` 1–6, `top` 1–100, and caps of 50,000 paths, 20 languages, 20 components, 20 boundaries/component and 400 globally, 50 entry points, 100 hotspots, 50,000 cycle edges, 50 cycles, and 500 memberships. `path_work_limit` marks partial path aggregates; `top_probe` marks hotspot and per-component boundary cap+1 probes; `global_probe_limit` marks uncertainty caused by the 401st global boundary row; cycle `work_limit` skips SCC analysis. |
| `mmcg_change_impact` | Full schema-v1 baseline/worktree analysis: changed symbols including body-only edits, bounded callers, component crossings, candidate tests, collection totals/truncation, and stable precision notes. Includes staged, unstaged, and untracked files. |
| `mmcg_test_impact` | Exact projection of `mmcg_change_impact` containing baseline, scope, changes, candidate tests, limits, and precision notes. Direct graph tests rank above transitive and same-component filename heuristics; focused candidates never replace the full gate. |
| `mmcg_change_class` | Classify a proposed change by blast-radius risk |
| `mmcg_recent_changes` | Files re-indexed within a time window |
| `mmcg_tasks` | Full-text search over task specs in `.mastermind/tasks/` |
| `mmcg_status` | Index health, freshness, and task summary |
| `mmcg_scratchpad_append` | Append a note to the persistent scratchpad |
| `mmcg_scratchpad_read` | Read the scratchpad (newest entries first) |

Full tool schemas are returned by the standard MCP `tools/list` call.

## Starting the server manually

```bash
mastermind serve
```

The server reads JSON-RPC from stdin and writes to stdout. It exits when stdin closes.

## Prerequisites

- Mastermind installed (`npm install -g @xcraftmind/mastermind` or built from source)
- Project indexed (`mastermind index .` run inside the project root)
- Index file at `.mastermind/mmcg.db` (or passed via `--index`)
