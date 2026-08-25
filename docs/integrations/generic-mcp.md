# Generic MCP client integration

If your client can launch a stdio MCP server, it can use Mastermind. This guide
gives you the smallest safe config and the exact protocol boundary when no
dedicated Claude Code, Codex, Cursor, or Continue setup command applies.

`mastermind serve` is the server process.

## Server spec

| property | value |
|---|---|
| transport | stdio |
| command | `mastermind serve` |
| protocol | MCP 2025-11-25; legacy 2024-11-05 |
| tools | 29: 20 refreshable non-destructive queries, 8 read-only queries, 1 additive local write (see below) |
| resources | none |
| prompts | none |

Current clients receive behavior annotations and structured results after the
initialized notification; legacy clients retain content-only results. Input
frames are limited to 1 MiB and serialized result payloads to 8 MiB.

## Preview first, then apply

Generic setup requires an explicit JSON path:

```bash
mastermind setup generic --scope project --config ./mcp.json        # dry-run
mastermind setup generic --scope project --config ./mcp.json --write
```

The command preserves unrelated root fields and MCP servers. Customized
replacement or removal requires `--force`; `--force` never implies `--write`.
Before a forced mutation, Mastermind stores previous bytes under
`~/.mastermind/setup-backups/`. `mastermind doctor` treats configuration as
bounded data and does not execute a configured command.

## Minimal config

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

## What the client receives

- Symbol discovery: `mmcg_search`, `mmcg_outline`, `mmcg_files`,
  `mmcg_symbols_in_file`.
- Relationships and architecture: `mmcg_callers`, `mmcg_callees`,
  `mmcg_imports`, `mmcg_imported_by`, `mmcg_impact`, `mmcg_api_surface`,
  `mmcg_centrality`, `mmcg_dependency_cycles`, `mmcg_unreferenced`,
  `mmcg_semantic`, `mmcg_facts`, `mmcg_team_map`, `mmcg_map`, `mmcg_temporal`.
- Change analysis: `mmcg_symbols_changed_since`, `mmcg_change_class`,
  `mmcg_change_impact`, `mmcg_brief`, `mmcg_test_impact`, `mmcg_recent_changes`.
- Workflow state: `mmcg_tasks`, `mmcg_history`, `mmcg_status`, `mmcg_scratchpad_read`, and the
  additive local write `mmcg_scratchpad_append`.

The standard MCP `tools/list` response is the schema source of truth. See the
[technical reference](../reference/mmcg.md#mcp-tools) for arguments, limits,
and precision caveats.

For role entry, prefer one `mmcg_brief` call with `role`, `since`, and an
optional `budget_tokens` (default 2,000). Current MCP transports duplicate its
logical packet into `content.text` and `structuredContent`; the reported budget
already counts both copies after JSON escaping. The packet marks repository
strings as untrusted data and never includes source bodies, declaration
signatures, literal/default values, or free-form history excerpts.

## Start the server manually

```bash
mastermind serve
```

The server reads JSON-RPC from stdin and writes to stdout. It exits when stdin closes.

## Prerequisites

- Mastermind installed (`npm install -g @xcraftmind/mastermind` or built from source)
- Project indexed (`mastermind index .` run inside the project root)
- Index file at `.mastermind/mmcg.db` (or passed via `--index`)
