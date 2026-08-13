# MCP servers

Mastermind ships one [Model Context Protocol](https://modelcontextprotocol.io)
server.

| Server | Transport | Surface |
|---|---|---|
| [`mmcg`](servers/mmcg/README.md) | stdio | 28 bounded tools over a local SQLite index: 27 read-only queries and one additive scratchpad write |

The server supports Python, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust, C#,
Go, Java, PHP, and C/C++. It exposes structural, semantic, evidence, history,
and workflow-state queries; it does not expose arbitrary SQL or executable
plugin hooks.

Start with the [generic MCP guide](../docs/integrations/generic-mcp.md) or the
[complete technical reference](../docs/reference/mmcg.md#mcp-tools).
