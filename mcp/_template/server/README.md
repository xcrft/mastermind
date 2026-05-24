---
name: <server-slug>
description: <One or two sentences. What the server exposes, when to use it.>
metadata:
  version: 0.1.0
  authors:
    - <github-handle>
  tags:
    - <domain>
  transport: <stdio | http | sse>
  source: <URL to the server's source repo>
---

# <Server Name>

<One-paragraph what-and-when.>

## Tools exposed

- `tool_name(arg1, arg2)` — <what it does>
- `another_tool(arg)` — <what it does>

## Install

```bash
<actual install commands — what the adopter copies and runs>
```

## Configure

Add to your MCP config (e.g., `~/.claude/mcp.json`):

```json
<contents of config.json — paste literally so the reader doesn't have to open the other file>
```

## Env vars

| Var | Required | Default | What it does |
|---|---|---|---|
| `VAR_NAME` | yes / no | <default or —> | <description> |

## Notes

<Optional. Version compatibility, gotchas, related servers.>
