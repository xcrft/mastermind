---
name: <your-slug>
description: <One or two sentences. What this subagent does, when to spawn it.>
# Runtime fields — Claude Code reads these at the TOP LEVEL, not under metadata.
tools: Read, Grep              # ONLY the tools this subagent needs; omit the line to inherit all
model: <opus | sonnet | haiku | inherit>
mcpServers: [mmcg]             # grant MCP servers — a tools: allowlist excludes MCP otherwise; delete if none
metadata:
  version: 0.1.0
  authors:
    - <github-handle>
  tags:
    - <domain>
---

# <Subagent Name>

<One-paragraph what-and-when. This becomes part of the system prompt the subagent sees.>

## Role

<Describe the subagent's role in 2-3 sentences. What is its job? What is it NOT supposed to do?>

## Inputs

<What does the spawner pass in? A file path? A question? A diff?>

## Process

1. <First step>
2. <Second step>
3. <…>

## Output

<What does the subagent return? Be specific about format — the spawner relies on this shape.>
