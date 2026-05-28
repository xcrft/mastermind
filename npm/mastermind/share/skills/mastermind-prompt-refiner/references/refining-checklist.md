# Refining checklist — what to look for, how to fix it

Reference for the [`mastermind-prompt-refiner`](../SKILL.md) skill. When refining a prompt, walk this list and apply the matching fix.

---

## Anti-patterns and fixes

| Smell | Why it bites | Fix |
|---|---|---|
| Vague verb (`help me with X`, `look at this`, `analyze`) | Model picks an arbitrary verb that may not match user's intent | Replace with a specific verb tied to the deliverable: `classify`, `extract`, `summarize in 3 bullets`, `produce a JSON with fields …` |
| No format specified | Output is unparseable / inconsistent across runs | State the output shape: length, structure, fields, schema |
| Multiple bundled intents (`analyze, then summarize, then recommend`) | Model rushes through each, none gets full attention | Split into a chain, or pick the primary intent and drop the rest |
| Hardcoded values that should vary (`for Q3 sales`) | Prompt isn't reusable; breaks next time | Replace with `{{PLACEHOLDER}}` and document the variable |
| No success criterion | Reviewer can't tell if output is good | Add one sentence: "Good output has X, Y, Z." or "Score ≥ N." |
| Over-constrained (10+ rules) | Constraints contradict, model becomes timid | Cut to the 3 that actually matter. Move the rest to "soft preferences" or drop. |
| Contradicts itself (`be detailed but keep it short`) | Model picks one and ignores the other | Surface the contradiction and ask the user, do not guess |
| Asks for hallucination (`predict what will happen`) | Model invents confident wrong answers | Reframe: "Give 2-3 plausible scenarios. For each: assumptions, expected outcome, confidence (high/med/low), what would invalidate it." |
| Encourages refusal (`How do I manipulate people?`) | Model refuses or hedges, real intent unmet | Clarify legitimate use case in the prompt itself |
| Wall of context, real task buried in the middle | Middle gets skipped; output ignores the task | Move task to the start AND restate at the end; long context goes in between |

---

## Before / after

### Vague → specific deliverable
**Before:** "Help me write a better prompt for analyzing feedback."
**After:** "Refine this prompt so it classifies feedback as positive/negative/neutral, extracts up to 3 themes, and outputs JSON `{sentiment, themes[], actions[]}`. Handles 50-500 word messages. Original prompt: <…>"

### Bundled → chained
**Before:** "Analyze this data, summarize it, and give me recommendations."
**After:**
- Stage 1: "Extract key metrics from the data as a markdown table."
- Stage 2: "Given the table from Stage 1, identify the top 3 trends with supporting numbers."
- Stage 3: "Given the trends, recommend up to 3 actions, each with expected impact and effort."

### No format → exact format
**Before:** "List the top products."
**After:** "List the top 5 products as JSON: `[{"name": string, "revenue_usd": int, "growth_pct": float}]`. Sort by `revenue_usd` desc."

### Encourages hallucination → calibrates honestly
**Before:** "What will the market do next year?"
**After:** "Based on the data provided, give 2-3 plausible scenarios for the market next year. For each: key assumptions, expected outcome, confidence (high/med/low), what would invalidate the scenario."

### Hardcoded → parameterized
**Before:** "Analyze Q3 2025 sales for the EMEA region."
**After:** "Analyze {{PERIOD}} sales for the {{REGION}} region." plus a Variables table documenting both placeholders.

### Wall of context → instruction-sandwiched
**Before:**
```
<3000 lines of code>
What's wrong with this?
```
**After:**
```
You are reviewing the following code for correctness, security, and design issues. Report findings in priority order: must-fix, should-fix, consider.

<3000 lines of code>

Now produce the review of the code above. Use the priority categories described.
```

---

## Decision tree: refine inline or ask first?

```
Is the user's goal clear?
├─ NO  → Ask 1-3 targeted clarifying questions. STOP. Don't refine yet.
└─ YES → Are there <= 3 small gaps (format / edge case / minor constraint)?
         ├─ YES → Refine inline. Mark unresolvable gaps with <NEEDS:>.
         └─ NO  → Ask 1-3 targeted questions about the biggest gaps. STOP.

Is the prompt already tight (verb + format + constraint + success criterion)?
└─ YES → Output unchanged. "No changes needed."
```

### Good clarifying questions

- **Specific.** "Improve onboarding for whom — end users, new employees, API consumers?" beats "What kind of onboarding?"
- **Decision-bearing.** The question's answer must change the refined prompt. Don't ask cosmetic questions.
- **Limited.** 1-3 questions. Wall-of-questions = give up and refine with `<NEEDS:>` markers instead.

### Bad clarifying questions

- "Can you tell me more about your use case?" (too vague)
- "What's the context?" (no decision attached)
- 5+ questions at once (the user came to be helped, not interrogated)
