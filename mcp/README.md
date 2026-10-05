# MCP server

[`mmcg`](servers/mmcg/README.md) exposes the local Mastermind index over stdio.
Clients can query code, project history, documentation, workflow state and
explicitly granted personal context.

| Tool behavior | Write boundary |
|---|---|
| Structural queries | May refresh the managed derived index |
| `mmcg_scratchpad_append` | Appends a local note |
| `mmcg_mining_submit` | Stages source-cited proposals from the current task |
| Other queries | Read-only |
| Arbitrary SQL or executable plugins | Not exposed |

- [Connect a client](../docs/integrations/generic-mcp.md)
- [Tool arguments and result contracts](../docs/reference/mmcg.md#mcp-tools)
- [Index coverage and limitations](../docs/reference/mmcg.md#what-it-indexes)
