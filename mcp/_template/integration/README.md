---
name: <integration-slug>
description: <One or two sentences. What this recipe accomplishes end-to-end.>
metadata:
  version: 0.1.0
  authors:
    - <github-handle>
  tags:
    - <domain>
    - integration
  requires:
    - mcp/servers/<server-1>
    - skills/<domain>/<skill-1>
---

# <Integration Name>

<One-paragraph what this combination of pieces does together.>

## What you'll set up

- <Server 1> — <why>
- <Skill 1> — <why>
- <Hook or prompt, if any>

## Steps

1. <Install server 1 — link to its README, don't duplicate install steps>
2. <Copy skill 1 into ~/.claude/skills/>
3. <Set env var X>
4. <Test with this prompt: "...">

## Verifying it works

<A concrete prompt and the expected behavior, so the adopter can confirm the wiring is right before relying on it.>

## Notes

<Optional. Limitations, alternatives, version compatibility.>
