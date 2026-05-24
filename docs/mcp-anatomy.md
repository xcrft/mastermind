# MCP anatomy

The `mcp/` tree holds **MCP (Model Context Protocol) server configs** and integration recipes. An MCP server exposes tools/resources to an agent over a stdio or HTTP protocol.

Read [`conventions.md`](conventions.md) first.

---

## Two kinds of artifacts

| Sub-folder | What it is |
|---|---|
| `servers/` | A specific MCP server config — what it is, how to install it, what tools it exposes. |
| `integrations/` | A recipe combining one or more MCP servers with prompts/skills to accomplish something. |

---

## 1. Server (`mcp/servers/<slug>/`)

### Layout
```
mcp/servers/<slug>/
├── README.md         # required: what this server does, install, env vars, tools list
├── config.json       # required: the snippet to merge into mcpServers config
└── scripts/          # optional: install/wrapper scripts
```

### README frontmatter
```yaml
---
name: filesystem-readonly
description: Read-only filesystem MCP server — exposes file read and listing tools without write access. Use when an agent needs to inspect files outside its working directory without risk of modification.
metadata:
  version: 0.1.0
  tags:
    - filesystem
    - readonly
  transport: stdio          # stdio | http | sse
  source: https://github.com/modelcontextprotocol/servers
---
```

### README body
```markdown
# <Server name>

<One-paragraph what-and-when.>

## Tools exposed

- `read_file(path)` — …
- `list_directory(path)` — …

## Install

```bash
<the actual commands>
```

## Configure

Add to your MCP config (e.g., `~/.claude/mcp.json` or `claude_desktop_config.json`):

\`\`\`json
<contents of config.json>
\`\`\`

## Env vars

| Var | Required | Default | What it does |
|---|---|---|---|
| `READ_ROOT` | yes | — | Root path the server is allowed to read from. |

## Notes

(Optional.) Gotchas, version compatibility, related servers.
```

### config.json shape
```json
{
  "mcpServers": {
    "filesystem-readonly": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "--readonly"],
      "env": {
        "READ_ROOT": "${READ_ROOT}"
      }
    }
  }
}
```

Use `${VAR_NAME}` for env-var placeholders the adopter must fill in.

---

## 2. Integration (`mcp/integrations/<slug>/`)

An integration is a recipe: "to do X, install these MCP servers, use this prompt, set this hook."

### Layout
```
mcp/integrations/<slug>/
├── README.md         # required: what this recipe accomplishes
└── (links to servers/, skills/, prompts/ — don't duplicate content)
```

### README frontmatter
```yaml
---
name: github-pr-triage
description: Wire up GitHub MCP + a pr-triage skill so an agent can fetch, label, and comment on PRs end-to-end. Use when setting up an agent for PR maintenance work.
metadata:
  version: 0.1.0
  tags:
    - github
    - integration
  requires:
    - mcp/servers/github
    - skills/code-review/pr-triage
---
```

Body lists the steps — install server X, copy skill Y, set env Z, test with this prompt.

---

## Reviewing an MCP PR

1. **Tool list is documented.** Every tool the server exposes appears in the README.
2. **Env vars are listed.** No surprise `getenv()` calls — every variable the server reads must be in the table.
3. **Install instructions actually work.** The reviewer should be able to follow them cold.
4. **No secrets in `config.json`.** Use `${VAR}` placeholders, not literal values.
5. **Transport stated.** `stdio` vs `http` vs `sse` — this matters for what works where.
