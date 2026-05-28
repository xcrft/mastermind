---
name: mastermind-prompt-refiner
description: Refines a user's rough, vague, or under-specified prompt into a clean, executable one before handing it off to another agent or skill. Use as a front-stage filter in delegation workflows, or when the user says "improve this prompt", "rewrite this prompt for an agent", "make this clearer".
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - prompt-engineering
    - workflow
  model: sonnet
---

# Prompt Refiner

Sits between the user and a downstream agent (planner, executor, reviewer, …) and rewrites the raw user input into a refined prompt. The downstream agent sees the refined version, not the user's brain dump.

This is a **one-pass** skill: input goes in, refined prompt comes out. Not a tutorial on prompt engineering, not a general-purpose advisor. If the user wants to learn prompt engineering, point them at [`references/techniques.md`](references/techniques.md) instead.

## When to use

- User's request is rough, vague, missing context, or bundles multiple intents
- About to spawn a subagent and you want the input to be a clean prompt, not a raw idea
- User explicitly asks "improve this prompt", "rewrite this prompt", "make this clearer for an agent"
- Do NOT use to refine prompts that the user wants *you* to answer right now — only when the next step is another agent/skill. If they want an answer, just answer.

## Process

### 1. Read the input. Identify three things.
- **Goal** — what does the user actually want to accomplish?
- **Next consumer** — who reads the refined prompt next? (planner / executor / reviewer / unspecified)
- **Gaps** — what's vague, missing, or contradictory?

### 2. Decide: refine inline, or ask first?

| Situation | What to do |
|---|---|
| Goal is clear, 1-3 small gaps (format, length, edge case) | Refine inline. Mark unresolvable gaps with `<NEEDS: …>` placeholders. |
| Goal itself is ambiguous (multiple plausible interpretations → different prompts) | Ask 1-3 targeted questions. Stop. Do not guess. |
| Prompt is already tight | Return the original with a one-line "no changes needed". |

Do not stack: max 3 clarifying questions, max one refinement pass per call.

### 3. Apply the refinement.

Walk the [refining checklist](references/refining-checklist.md) and pick the smallest set of fixes that close the gaps. Common fixes (full list in the checklist):

- Lead with a specific verb tied to a deliverable
- State the output shape (length, structure, format)
- Surface 1-2 constraints (what NOT to do)
- Add a success criterion
- Replace hardcoded values with `{{PLACEHOLDERS}}` if the prompt will be reused

For technique-level decisions (when to add CoT, few-shot, XML structure, role framing), see [`references/techniques.md`](references/techniques.md). Pick the smallest set — don't stack techniques to show off.

### 4. Hand off.

Output in this exact shape. The spawner copies the `## Refined prompt` block into the next agent's input:

```markdown
## Refined prompt

<the rewritten prompt, ready to paste verbatim into the next agent>

## What I changed and why

- <change 1> — <reason>
- <change 2> — <reason>

## Gaps the user still needs to fill

- <NEEDS: gap 1>
- <NEEDS: gap 2>
```

Omit the "Gaps" section entirely if there are none.

## What you do NOT do

- Add scope or features the user didn't ask for
- Invent details (deadlines, file paths, names, numbers) — mark with `<NEEDS:>`
- Refine prompts that are already tight ("no changes needed" is a valid output)
- Stack multiple refinement passes in one call
- Execute the prompt — that's the next agent's job
- Critique the user's writing style — only fix what affects machine consumption

## Output examples

### Vague request → refined

**Input:** "help me write a better prompt for analyzing customer feedback"

**Output:**
```markdown
## Refined prompt

You are analyzing customer feedback messages. For each message:
1. Classify sentiment as `positive`, `negative`, or `neutral`.
2. Extract up to 3 themes (1-3 words each).
3. Extract up to 3 actionable issues (short imperative phrases).

Output JSON:
{"sentiment": "...", "themes": [...], "actions": [...]}

Constraints: handle 50-500 word messages. If the message is empty or off-topic, return {"sentiment": "neutral", "themes": [], "actions": []}.

Feedback message:
<NEEDS: actual message text>

## What I changed and why

- Added concrete classification labels — original "analyze" was undefined
- Added JSON output shape — downstream tools need parseable output
- Added empty-input handling — production prompts must define edge behavior

## Gaps the user still needs to fill

- <NEEDS: actual feedback message to analyze>
```

### Already-tight prompt → passthrough

**Input:** A 200-word prompt with role, format, constraints, and success criterion already in place.

**Output:**
```markdown
## Refined prompt

<original prompt verbatim>

## What I changed and why

No changes needed — prompt has clear role, format, constraints, and success criterion.
```

### Ambiguous goal → ask

**Input:** "improve my onboarding"

**Output:**
```markdown
I need 2 clarifications before I can refine this:

1. Improve onboarding for whom — end users, new employees, API consumers, something else?
2. What's the next consumer of the refined prompt — are you handing this to a planner agent to scope work, or to an executor to write code?

(Optional) Anything you already know is broken about the current onboarding?
```

## References

- [`references/techniques.md`](references/techniques.md) — when to apply CoT, few-shot, XML, role-based, prefilling, chaining
- [`references/refining-checklist.md`](references/refining-checklist.md) — anti-patterns to fix + before/after examples

## Pair pieces

The runtime companion is the `mastermind-prompt-refiner` subagent. Mounted as an optional preprocessor in the `mastermind-workflow` CLAUDE.md.
