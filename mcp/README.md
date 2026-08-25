# MCP servers

One local graph, one bounded [Model Context Protocol](https://modelcontextprotocol.io)
server, no arbitrary SQL surface.

Mastermind currently ships one server:

| Server | Transport | Surface |
|---|---|---|
| [`mmcg`](servers/mmcg/README.md) | stdio | 29 bounded tools over a local SQLite index: 20 refreshable non-destructive queries, 8 read-only queries, and one additive scratchpad write |

The server supports Python, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust, C#,
Go, Java, PHP, and C/C++. It exposes structural, semantic, evidence, history,
and workflow-state queries; it does not expose arbitrary SQL or executable
plugin hooks.

Want to connect a client? Start with the
[generic MCP guide](../docs/integrations/generic-mcp.md). Need exact schemas,
limits, and precision notes? Use the
[complete technical reference](../docs/reference/mmcg.md#mcp-tools).
