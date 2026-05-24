# MCP

[Model Context Protocol](https://modelcontextprotocol.io) server configs and integration recipes.

| Sub-folder | What it is |
|---|---|
| [`servers/`](servers/) | Specific MCP server configs — what it does, install, env vars, tools list. |
| [`integrations/`](integrations/) | Recipes combining MCP servers with skills/prompts for end-to-end workflows. |

See [`../docs/mcp-anatomy.md`](../docs/mcp-anatomy.md) for the format.

## Index

### servers/
| Server | Transport | Description |
|---|---|---|
| [`filesystem-readonly`](servers/filesystem-readonly/README.md) | stdio | Read-only filesystem access — list directories and read files, no writes. |
| [`mmcg`](servers/mmcg/README.md) | stdio | Mastermind Codegraph — fast Python/TS/JS/Rust code indexer with fully-qualified import paths. 8 structural query tools (search, callers, callees, impact, imports, imported_by, files, status) + file watcher. Truth layer for the Mastermind workflow. |

### integrations/
*(none yet — contribute one!)*

---

## Adding a new MCP artifact

1. Read [`../docs/mcp-anatomy.md`](../docs/mcp-anatomy.md).
2. For a server: copy `_template/server/` to `mcp/servers/<your-slug>/`. For an integration: copy `_template/integration/`.
3. Fill in README and `config.json`. Test the install steps cold.
4. Add to this index.
5. Open a PR.
