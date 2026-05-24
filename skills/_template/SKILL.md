---
name: <your-slug>
description: <One or two sentences. Lead with the verb. State when to use it. List concrete triggers. See ../../docs/conventions.md §2.2.>
metadata:
  version: 0.1.0
  authors:
    - <github-handle>
  tags:
    - <domain>
  model: <opus | sonnet | haiku — optional, only if it matters>
  requires:
    - <tool or MCP server — optional>
---

# <Skill Name>

<One-paragraph what-and-when. This is read after the description triggers — use it to confirm the agent has the right skill.>

## When to use

- <Concrete trigger 1>
- <Concrete trigger 2>
- Do NOT use for <X> — use [[other-skill]] instead. (Only if there's a common confusion.)

## Prerequisites

<Optional. List required tools, env vars, MCP servers. Delete this section if none.>

## Steps

1. <First step, imperative.>
2. <Second step.>
3. <…>

## Outputs

<What the user sees after the skill runs. A report? A diff? A list of issues? Be specific.>

## Examples

<Optional but strongly encouraged. Show one realistic invocation and what it produces.>
