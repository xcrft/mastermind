---
name: context7
description: Opt-in recipe that gives the researcher and executor live, version-specific library documentation via the context7 MCP server — current API docs the model's training data can't have. Portable to any project with dependencies, but hosted (it breaks the offline default), so you add it deliberately, not by default.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - mcp
    - integration
    - docs
  requires:
    - agents/subagents/mastermind-researcher
    - agents/subagents/mastermind-task-executor
---

# context7 — live library docs

[context7](https://github.com/upstash/context7) injects up-to-date, version-specific
documentation and code examples for public libraries into the agent's context. It's the
highest-value addition to the [portable baseline](portable-baseline.md) — useful on any
project with dependencies, no org account — but it is **hosted**: every query leaves the
machine. That breaks mmcg's offline guarantee, so context7 is **opt-in**, never bundled
into the default install.

When it earns its keep: working against fast-moving libraries (Next.js, the Vercel AI
SDK, anything where the API shifted after the model's training cutoff). When it doesn't:
a stable, single stdlib — train-memory is fine, and you keep the offline property.

## What you'll set up

- The `context7` MCP server — two tools: `resolve-library-id` (name → context7 ID) and
  `query-docs` (fetch docs for that ID).
- Scoped to the roles that gather/apply facts: `mastermind-researcher` (mmcg-first for
  structure, context7 for library docs) and `mastermind-task-executor`.

## Steps

1. **Register the server** in the project `.mcp.json`, alongside the baseline `mmcg`.

   Local (stdio):
   ```json
   {
     "mcpServers": {
       "mmcg":     { "command": "mastermind", "args": ["serve"] },
       "context7": { "command": "npx", "args": ["@upstash/context7-mcp"], "env": { "CONTEXT7_API_KEY": "${CONTEXT7_API_KEY}" } }
     }
   }
   ```

   Or remote (HTTP):
   ```json
   { "mcpServers": { "context7": { "url": "https://mcp.context7.com/mcp" } } }
   ```

   `CONTEXT7_API_KEY` is **optional** — a free key from `context7.com/dashboard` raises
   rate limits. Omit it to start.

2. **Scope it to the roles** (top-level `mcpServers:`, alongside `mmcg`):

   ```yaml
   # agents/subagents/mastermind-researcher.md
   tools: Read, Grep, Glob, Bash
   model: haiku
   mcpServers: [mmcg, context7]
   ```

   Same for `mastermind-task-executor`. Leave the critic/auditor/planner on `mmcg` only —
   they verify structure, they don't need external docs.

3. **Verify**: `mastermind doctor` — the `subagent MCP scoping` check confirms `context7`
   is registered for every role that scopes it.

## Verifying it works

Spawn the researcher on a doc question whose answer changed recently — e.g. "what's the
current `generateText` signature in the Vercel AI SDK?" It should call
`resolve-library-id` → `query-docs` and return version-current docs rather than
training-era memory. If context7 were unregistered, it'd silently fall back to web/grep.

## Notes

- **Opt-in by design.** This is the one place the portable layer reaches the network.
  Adopting it is a conscious trade of the offline guarantee for current docs — make it
  per-project, never the default.
- **Roles only.** Granting context7 to roles that don't read docs is pure friction (extra
  tools in context for no gain) — keep it on researcher/executor.
- mmcg answers "what's in THIS repo"; context7 answers "what's in the libraries this repo
  depends on". They're complementary, not redundant.
