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
| [`mmcg`](servers/mmcg/README.md) | stdio | Mastermind Codegraph — 20 structural query tools over a local SQLite index. Nine languages: Python, TS/JS, Rust, C#, Go, Java, PHP, C/C++. Truth layer for the Mastermind workflow. |

### integrations/
| Integration | Combines | Description |
|---|---|---|
| [`portable-baseline`](integrations/portable-baseline.md) | mmcg (+ context7 opt-in) | The project-agnostic MCP layer carried into every repo — local, offline, no org accounts. |
| [`org-overlay`](integrations/org-overlay.md) | portable-baseline + per-project SaaS MCP | Layer a repo's own observability / tracker / chat / design MCP onto the baseline and scope each to the subagent role that needs it. |
| [`context7`](integrations/context7.md) | portable-baseline + context7 | Opt-in live library docs for the researcher/executor — current API docs the model can't have. Hosted, so outside the offline default. |

---

## Adding a new MCP artifact

1. Read [`../docs/mcp-anatomy.md`](../docs/mcp-anatomy.md).
2. For a server: copy `_template/server/` to `mcp/servers/<your-slug>/`. For an integration: copy `_template/integration/`.
3. Fill in README and `config.json`. Test the install steps cold.
4. Add to this index.
5. Open a PR.
