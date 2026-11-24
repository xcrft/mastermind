# Generic MCP integration

Install Mastermind and index the repository. The client must launch a stdio
MCP server.

## Configure the server

For JSON `mcpServers` configuration, use the client's actual configuration path:

```bash
mastermind setup generic --scope project --config ./mcp.json
mastermind setup generic --scope project --config ./mcp.json --write
```

The first command previews a merge that preserves unrelated settings.
Equivalent manual configuration:

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

| Setting | Value |
|---|---|
| Working directory | Repository root |
| npm command | `mastermind` or `mmcg` |
| Cargo command | `mmcg` |
| Apply registration | Restart the client |

To select a specific index, place `--index` before `serve`:

```json
{
  "mcpServers": {
    "mmcg": {
      "command": "mastermind",
      "args": ["--index", "/absolute/path/project/.mastermind/mmcg.db", "serve"]
    }
  }
}
```

## Choose a tool

| Need | Start with |
|---|---|
| Role briefing | `mmcg_brief` |
| Task/agent effort decision | `mmcg_effort_route` |
| Name or concept | `mmcg_search`, `mmcg_concept` |
| Operation relevance | `mmcg_concept_prepare`, source reads, `mmcg_concept_finish` |
| Structure or change impact | `mmcg_map`, `mmcg_change_impact` |
| Project documentation | `mmcg_docs`, `mmcg_project_profile` |
| Combined context | `mmcg_context` |
| Task records | `mmcg_tasks`, `mmcg_history` |
| Granted personal preferences | `mmcg_profile` |

`tools/list` defines names and schemas.
For concept review, call `mmcg_concept_prepare`, inspect the returned source,
then submit judgments with `mmcg_concept_finish`. The response carries the role
rubric. The request contract, witness and role rules are in
[Local symbol concept search](../reference/mmcg.md#local-symbol-concept-search).
The [MCP reference](../reference/mmcg.md#mcp-tools) defines arguments and limits.
Graph queries may refresh the derived index. Scratchpad append is an additive
local write.

The effort tool returns a requested level and client argument arrays. Apply it
through a supported per-agent control before the next invocation; without that
control, keep the existing setting. It does not change this chat or launch a
model. See [effort routing](../guides/effort-routing.md).

## Enable personal context separately

```bash
mastermind miner access grant . --client my-client
```

| Requirement | Value |
|---|---|
| Read grant | This repository and client ID |
| Server environment | `MMCG_PROFILE_CLIENT=my-client` |
| Profile behavior | Advisory, live source checks |
| Access denied | Withhold personal context, no static `style.md` fallback |

See [Persona hooks](../guides/persona-hooks.md) for capture and review.

## Protocol and removal

| Protocol property | Value |
|---|---|
| Transport | JSON-RPC over stdin/stdout |
| MCP versions | See the [protocol contract](../reference/mmcg.md#protocol-contract) |
| Catalog | Tools, no resources or prompts |

Source and message bounds: [protocol contract](../reference/mmcg.md#protocol-contract).

Remove the entry with the same setup path and `--remove`. Inspect the preview,
then add `--write`. Customized entries require `--force`. Forced file changes
save backups under `~/.mastermind/setup-backups/`.
