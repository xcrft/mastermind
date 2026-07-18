---
name: mastermind-prompt-refiner
description: Rewrite a request for a different agent or reusable prompt while preserving the original intent verbatim. Use when the user explicitly asks to improve/rewrite a prompt, or immediately before handing a genuinely rough request to a cold agent that will not see the original conversation. Do not activate merely because the current request is informal or multi-sentence.
metadata:
  version: 0.4.0
  authors: [mastermind]
  tags: [prompt-engineering, workflow]
---

# Prompt refiner

Produce a bounded handoff without silently replacing the user's source request.
The next consumer receives both the original text and a normalized brief, so
constraints lost in rewriting remain recoverable.

## When to use

- The user explicitly asks to improve, rewrite, or package a prompt.
- A cold agent will receive no conversation history and the raw request is
  genuinely ambiguous, contradictory, or bundles separate deliverables.
- A reusable prompt needs placeholders and a stable output contract.

Do not activate just because the user writes informally. If the current agent
can answer or implement the request with the available context, do that. A
planner-ready request should pass through unchanged.

## Decision

| Input | Action |
|---|---|
| Clear goal, deliverable, scope, and success condition | `passthrough` |
| Clear goal or several clear deliverables with handoff-blocking gaps | `refined` |
| One requested outcome has multiple incompatible interpretations | `ask` |

Ask at most three questions and only when the answer changes the deliverable,
permission boundary, or workflow. Do not manufacture file paths, deadlines,
features, risk decisions, or acceptance criteria.

## Refinement method

1. Identify the requested outcome and next consumer.
2. Separate distinct deliverables instead of choosing one silently.
3. Preserve explicit constraints, exclusions, permissions, and prior decisions.
4. Add only the minimum output shape and success condition required by the next
   consumer.
5. Mark unresolved inputs with `<NEEDS: ...>`.
6. Keep the original request verbatim in the output.

Several explicit deliverables are not an ambiguous goal. Preserve each as a
separate workstream, state their scope boundary, and mark priority/dependency
questions with `<NEEDS: ...>`; use `action: refined`. Use `ask` only when the
requested outcome itself cannot be normalized without choosing incompatible
meanings on the user's behalf.

For migrations, destructive operations, credential changes, or other hard-to-
reverse work, do not choose a workflow mode. Surface missing rollback, backup,
approval, and recovery inputs when they materially affect whether the next
agent can plan safely.

Technique references are optional tools, not a checklist to stack:
[`references/refining-checklist.md`](references/refining-checklist.md) and
[`references/techniques.md`](references/techniques.md).

## Output contract

For `refined` and `passthrough`:

````markdown
## Original request

<original text verbatim>

## Refined prompt

<handoff-ready prompt; identical to original for passthrough>

## What changed

- <material normalization and reason, or “No changes needed.”>

## Unresolved inputs

- <NEEDS: input>

<!-- mastermind:intake-begin -->
```yaml
action: refined
target_consumer: planner
```
<!-- mastermind:intake-end -->
````

Omit `Unresolved inputs` when empty. For `ask`, use exactly these sections and
do not emit a `Refined prompt` heading or commentary about omitting one:

````markdown
## Original request

<original text verbatim>

## Questions

1. <question that changes the deliverable or boundary?>

<!-- mastermind:intake-begin -->
```yaml
action: ask
target_consumer: planner
```
<!-- mastermind:intake-end -->
````

The intake block is routing evidence for a caller that chooses to parse it. It
does not select Direct/Verified/Strict, spawn research or critics, authorize an
executor, or replace planner judgment.

## Boundaries

- Never discard or paraphrase away the original source request.
- Never route raw intent directly to executor unless the caller provides an
  approved spec.
- Never execute the refined prompt in refiner mode.
- Never add scope to make the prompt look complete.
- One pass only; if the goal remains ambiguous, ask rather than iterating on
  invented assumptions.
