---
name: api-shape-explorer
description: Generate three radically different API/interface shapes for the same problem so you can compare tradeoffs before committing. Use when you're about to design a new module, service boundary, or public API and want to avoid anchoring on the first idea.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - workflow
    - design
  role: user
  variables:
    - name: PROBLEM
      required: true
      description: What the API needs to do. Concrete use cases, not abstract goals.
    - name: CONSTRAINTS
      required: false
      description: Hard constraints — language, framework, latency, compatibility.
---

# API Shape Explorer

A user prompt that asks the model to generate three meaningfully different interface designs for the same problem, with tradeoffs. The goal is to force a comparison before you anchor on the first shape that comes to mind.

## When to use

- Designing a new module's public API
- Choosing between an SDK shape, a CLI shape, and a config-file shape
- Service boundary discussions — "should this be one endpoint or three?"
- Before writing a design doc, to populate the "alternatives considered" section
- Do NOT use this for implementation — use it only at the interface-design stage

## Variables

| Name | Required | Description |
|---|---|---|
| `PROBLEM` | yes | What the API needs to do. Concrete use cases beat abstract goals. |
| `CONSTRAINTS` | no | Hard constraints — language, framework, latency targets, compatibility requirements. |

## Prompt

```text
I'm designing an API/interface for the following problem. Before I commit to one shape, I want to see three meaningfully different options.

PROBLEM:
{{PROBLEM}}

{{#if CONSTRAINTS}}
CONSTRAINTS:
{{CONSTRAINTS}}
{{/if}}

Generate exactly three interface designs. They must be *qualitatively* different — not three variations of the same idea. Examples of "qualitatively different": object-oriented vs. functional vs. data-pipeline; sync vs. async vs. streaming; one big call vs. many small calls vs. configuration-driven.

For each, give me:

## Option <N>: <short name>

**Shape:** A code snippet showing how a caller would actually use it. 10-20 lines, realistic.

**Mental model:** One sentence describing how a user has to think about this API to use it correctly.

**Strengths:**
- <thing 1>
- <thing 2>

**Weaknesses:**
- <thing 1>
- <thing 2>

**Best when:** <one sentence on the situation this is the right shape for>

After the three options, give me a short comparison:

## Comparison

| Dimension | Option 1 | Option 2 | Option 3 |
|---|---|---|---|
| Learning curve | low / medium / high | … | … |
| Composability | … | … | … |
| Testability | … | … | … |
| Extensibility | … | … | … |

End with one paragraph: "If forced to pick one without more information, I'd choose Option X because…"

Do not hedge. Do not say "all three have merit." Pick one and defend it.
```

## Example invocation

```text
I'm designing an API/interface for the following problem. Before I commit to one shape, I want to see three meaningfully different options.

PROBLEM:
A library that retries flaky operations. Used in Python services. Needs to support: exponential backoff, max attempts, retry only on specific exception types, callback on retry, async and sync code paths. Typical caller wraps a single function call.

CONSTRAINTS:
- Python 3.11+, no external runtime deps
- Must work in both sync and async code (one library, not two)
- Should be debuggable — users have to be able to see *why* a retry happened
```

## Notes

- The prompt deliberately forbids "all three are good" answers. Forcing a pick exposes which dimensions matter most to the model.
- Re-run with the same `PROBLEM` and a slightly different `CONSTRAINTS` to see how the recommendation shifts. That's often where the real insight is.
- Works well with Opus. Sonnet tends to make the three options closer to each other than they should be.
