---
name: portable-baseline
description: The project-agnostic MCP layer Mastermind carries into every repo — a local codegraph (mmcg) plus an opt-in for live library docs. No org accounts, works offline, identical on any project. Use when adopting Mastermind on a new repo and deciding what MCP to wire by default vs per-project.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - mcp
    - integration
  requires:
    - mcp/servers/mmcg
---

# Portable MCP baseline

The MCP layer that travels with the Mastermind workflow to **any** repository. The
rule for what belongs here: a tool is portable when it is **local**, reads **public
data**, or speaks a **protocol standard** (git/GitHub). Anything behind a company
SaaS account is *not* portable — it goes in the [`org-overlay`](org-overlay.md),
declared per-project.

The same subagents and skills then drop into every project unchanged; only the
per-project `.mcp.json` differs.

## What's in it

| Tool | Default? | Why it's portable |
|---|---|---|
| [`mmcg`](../servers/mmcg/README.md) | **yes** | local SQLite codegraph, no account, nine languages — the truth layer the workflow is built on |
| `context7` (live library docs) | opt-in | public docs for any project's dependencies; no org auth, but hosted — see [`context7`](context7.md) for the recipe |
| `filesystem-readonly` | situational | [generic scoped file access](../servers/filesystem-readonly/README.md) when an agent needs to read outside its working dir |

Browser drivers and `WebSearch`/`WebFetch` are portable too, but they're session
tools rather than registered MCP servers, so they need no `.mcp.json` entry.

## The portable `.mcp.json`

Committed with the repo, version-pinned. On a fresh project this is the whole file:

```json
{
  "mcpServers": {
    "mmcg": { "command": "mastermind", "args": ["serve"] }
  }
}
```

`mastermind setup claude --write-mcp` writes the `mmcg` entry for you (user scope, in
`~/.claude.json`). The org servers get appended to this same `mcpServers` object —
that's the overlay.

## Keep the default offline

mmcg never leaves the machine — the index is built locally by tree-sitter and queried
over stdio. **Do not bake a hosted MCP (context7, any SaaS) into the default install.**
The offline guarantee is a real property of the baseline; adding a server that phones
home silently revokes it for every adopter. Hosted servers are always opt-in recipes,
never the default.

## Verifying it works

```bash
mastermind doctor
```

Green `MCP config` (mmcg registered) and `subagent MCP scoping` (every subagent
`mcpServers:` entry resolves to a registered server) confirm the baseline is wired.
Then ask the agent "who calls `parseConfig`?" — the answer comes from the codegraph.

## Notes

- For org-specific tooling (observability, issue tracker, chat, design), see
  [`org-overlay`](org-overlay.md).
- ROI is not uniform: mmcg earns its keep on large or polyglot repos; on a small
  single-language repo it's convenience over grep. Add tools because the data is
  otherwise unreachable, not for completeness.
