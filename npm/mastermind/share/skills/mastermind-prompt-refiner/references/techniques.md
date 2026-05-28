# Refinement techniques — when to reach for each

Reference for the [`mastermind-prompt-refiner`](../SKILL.md) skill. Apply the smallest set of techniques that closes the gaps in the input prompt — do not stack them just because they exist.

---

## Chain-of-Thought (CoT)

**Reach for it when:** the task needs multi-step reasoning that models otherwise short-circuit. Math, multi-clause logic, comparative analysis.

**How:** add an explicit step list before the answer.

```
Work through this step by step:
1. Identify <X>
2. Compute <Y> from X
3. Cross-check against <Z>
Then state the final conclusion in one sentence.
```

**Skip when:** the task is single-shot retrieval, classification, or formatting. CoT adds latency and tokens without value.

---

## Few-shot examples

**Reach for it when:** the output format keeps drifting, or the task pattern is hard to describe abstractly but easy to show.

**How:**
- **1 example** — simple, clear-format tasks
- **2-3 examples** — moderately complex patterns or formats
- **5+ examples** — genuine class-diverse tasks (classification with many classes, edge-heavy parsing)

Examples should:
- Cover the format you want exactly
- Include at least one edge case (empty, malformed, boundary)
- Use realistic content. No `foo`/`bar`.

**Skip when:** the format is already specified explicitly and the model is following it.

---

## XML tags for structure

**Reach for it when:** the prompt has 3+ distinct sections (task, context, constraints, format, examples) and they're getting confused.

**How:**
```xml
<task>...</task>
<context>...</context>
<constraints>...</constraints>
<format>...</format>
```

**Skip when:** the prompt is short. XML adds noise to short prompts; helpful in long ones.

---

## Role-based framing

**Reach for it when:** the model's default style doesn't match the task. Wants engineering rigor, getting marketing fluff. Wants concise output, getting verbose output.

**How:**
```
You are a <role> with expertise in <domain>. Your priorities, in order:
1. <priority 1>
2. <priority 2>
3. <priority 3>
```

Write `you are`, not `act as if you were`. Direct framing works better than hypothetical.

**Skip when:** the task is mechanical (classify, extract, format) and tone doesn't matter.

---

## Prefilling

**Reach for it when:** the output format is rigid and the model keeps adding preamble or deviating.

**How:** end the prompt with the literal beginning of the desired output.

```
Output:

{
  "result":
```

The model continues from there. Works especially well for JSON / structured output.

**Skip when:** the output is free-form prose where you want the model to choose framing.

---

## Prompt chaining

**Reach for it when:** the task has clearly separate stages whose intermediate outputs you want to inspect or transform.

**How:** split into N prompts where output N feeds input N+1. Examples:
- extract → analyze → summarize
- classify → route → respond
- draft → critique → revise

**The refiner's job is to *identify* when chaining is appropriate** — not to do the splitting. If splitting is needed, the refiner says so in "What I changed and why" and the spawner sets up the chain.

**Skip when:** the task is genuinely single-step. Premature chaining costs latency.

---

## Context window discipline

**Always:** put the most important instruction last, just before the output. Models attend most strongly to the start and end of the prompt; middle content can be skipped.

**Always:** if the prompt includes a long document (data, code, examples), put it in the middle, between the instruction and a brief restatement of what to do with it.

```
<task instruction>

<long document>

Now, given the document above, <restate the specific task>.
```

---

## Combining techniques

Order matters. A typical refined prompt is:

```
<role framing — if needed>
<task statement, with the verb leading>
<context / inputs>
<step structure — if CoT needed>
<examples — if few-shot needed>
<format spec>
<constraints (what NOT to do)>
<final restatement of the task>
<prefill — if rigid format needed>
```

Not every refinement needs every layer. A 3-line prompt with the right verb and format spec beats a 50-line prompt with every technique applied.
