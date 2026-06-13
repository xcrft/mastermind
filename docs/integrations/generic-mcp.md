# Generic MCP client integration

`mastermind serve` is a standard [MCP](https://modelcontextprotocol.io) stdio server. Any MCP-capable client can connect to it.

## Server spec

| property | value |
|---|---|
| transport | stdio |
| command | `mastermind serve` |
| protocol | MCP 2024-11-05 |
| tools | 20 (see below) |
| resources | none |
| prompts | none |

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
