# Generic MCP client integration

`mastermind serve` is a standard [MCP](https://modelcontextprotocol.io) stdio server. Any MCP-capable client can connect to it.

## Server spec

| property | value |
|---|---|
| transport | stdio |
| command | `mastermind serve` |
| protocol | MCP 2025-11-25; legacy 2024-11-05 |
| tools | 24: 23 read-only, 1 additive local write (see below) |
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

- Symbol discovery: `mmcg_search`, `mmcg_outline`, `mmcg_files`,
  `mmcg_symbols_in_file`.
- Relationships and architecture: `mmcg_callers`, `mmcg_callees`,
  `mmcg_imports`, `mmcg_imported_by`, `mmcg_impact`, `mmcg_api_surface`,
  `mmcg_centrality`, `mmcg_dependency_cycles`, `mmcg_unreferenced`, `mmcg_map`.
- Change analysis: `mmcg_symbols_changed_since`, `mmcg_change_class`,
  `mmcg_change_impact`, `mmcg_test_impact`, `mmcg_recent_changes`.
- Workflow state: `mmcg_tasks`, `mmcg_history`, `mmcg_status`, `mmcg_scratchpad_read`, and the
  additive local write `mmcg_scratchpad_append`.

The standard MCP `tools/list` response is the schema source of truth. See the
[technical reference](../reference/mmcg.md#mcp-tools) for arguments, limits,
and precision caveats.

## Starting the server manually

```bash
mastermind serve
```

The server reads JSON-RPC from stdin and writes to stdout. It exits when stdin closes.

## Prerequisites

- Mastermind installed (`npm install -g @xcraftmind/mastermind` or built from source)
- Project indexed (`mastermind index .` run inside the project root)
- Index file at `.mastermind/mmcg.db` (or passed via `--index`)
