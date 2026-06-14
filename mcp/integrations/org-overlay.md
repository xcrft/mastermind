---
name: org-overlay
description: Recipe for layering a project's own SaaS MCP servers (observability, issue tracker, chat, design) onto the portable Mastermind baseline — declared per-project in `.mcp.json` and scoped to the subagent roles that need them via top-level `mcpServers:`. Use when a repo has org-specific tooling the workflow roles should reach.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - mcp
    - integration
  requires:
    - mcp/integrations/portable-baseline
---

# Org MCP overlay

The [portable baseline](portable-baseline.md) is the same everywhere. What differs
per repo is the **org layer** — the SaaS MCP servers behind a company account. They're
declared in that project's `.mcp.json` and handed only to the subagent roles that
actually use them. One repo gets Datadog + Linear; another gets nothing; the workflow
itself is unchanged.

This is the deliberate split: **never hardcode a company's stack into the standard.**

## Which class of tool feeds which role

Map by *class*, not by vendor — substitute your own (Datadog↔Honeycomb, Linear↔Jira):

| Role | Tool class | Why this role, not another |
|---|---|---|
| `mastermind-investigator` | observability (Datadog, PagerDuty, Sentry) | root-cause needs live prod logs/metrics/traces the model can't guess |
| `mastermind-researcher` | docs / knowledge (context7, Confluence, Guru) | bulk fact-gathering over current library docs + internal runbooks |
| planner / `mastermind-prompt-refiner` | tracker + chat (Linear, Jira, Slack, Notion) | pulls real decisions/ticket history into the brief instead of guessing |
| `mastermind-auditor` | code graph (`mmcg`) | already in the baseline; verification is structural, not SaaS |
| a verify/QA role | design + browser (Figma, Chrome) | sees the running UI / the source design — irreplaceable for frontend |

Highest ROI is observability and proprietary context: data the model has **no other
way** to reach. Convenience wrappers over `gh`/CLI/web are low ROI — skip them.

## Steps

1. **Register the server** in the project `.mcp.json` (alongside the baseline `mmcg`):

   ```json
   {
     "mcpServers": {
       "mmcg":    { "command": "mastermind", "args": ["serve"] },
       "datadog": { "command": "npx", "args": ["-y", "@your-org/datadog-mcp"], "env": { "DD_API_KEY": "${DD_API_KEY}" } }
     }
   }
   ```

2. **Scope it to the role** that needs it. A subagent with a `tools:` allowlist
   excludes all MCP unless the server is named in top-level `mcpServers:`
   (see [`agent-anatomy.md`](../../docs/agent-anatomy.md) §1):

   ```yaml
   ---
   name: mastermind-investigator
   tools: Read, Grep, Glob, Bash
   model: sonnet
   mcpServers: [mmcg, datadog]
   metadata: { version: 0.1.1, tags: [workflow, debugging] }
   ---
   ```

   Only the roles that use a server get it — don't grant Datadog to the researcher.

3. **Verify**: `mastermind doctor`. The `subagent MCP scoping` check warns if any
   subagent's `mcpServers:` names a server missing from `.mcp.json` / `~/.claude.json`
   — catching "scoped datadog to investigator but never registered it".

## Verifying it works

After wiring, ask the relevant role its question — e.g. spawn the investigator on a
real incident and confirm it pulls live telemetry rather than reasoning from the code
alone. If the server were unregistered or unscoped, the role would silently fall back
to grep/read; `doctor` is the deterministic pre-check.

## Notes

- **YAGNI.** Write an org recipe only when a concrete repo needs it; don't pre-build
  vendor integrations into the standard. Each MCP server in a subagent is context plus
  friction.
- **Offline.** Every server here phones home — that's the point, but it's why they live
  in the per-project overlay and never in the [default baseline](portable-baseline.md).
- Plugin-scoped subagents ignore `mcpServers:` (a Claude Code security rule). Mastermind
  installs subagents to `~/.claude/agents/` (user scope), where it is honored.
