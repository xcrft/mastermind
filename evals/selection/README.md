# Skill selection

A host picks a skill from its name and description alone, before it reads the
body. This directory checks that choice in two ways:

- `python -m evals.selection` runs without a model and is part of `just check`.
- The `selection` suite in [behavioural evals](../behavior/README.md) runs the
  same prompts through Claude when you want to measure what a model picks.

The prompts live in [`../behavior/cases/selection.jsonl`](../behavior/cases/selection.jsonl).
[`catalogue.md`](catalogue.md) is the system prompt for model runs. It is built
from every `SKILL.md`, so run `python -m evals.selection --write` after you change
a description, or the check fails.

## What the deterministic check proves

It proves that the corpus and the descriptions are consistent with each other:

- Every prompt expects one skill, a declared tie of skills, or `none`, and
  rejects at least one near miss. Every name is a shipped skill, and no prompt
  contains the name of the skill it expects.
- Every core skill is expected by at least two prompts and rejected by at
  least two. Every other skill has at least one of each.
- Every `(use X)` in a description names a shipped skill. No three or more
  skills hand a prompt on in a loop. A pair that names each other is fine: that
  is one boundary written from both sides.
- A conflict case (`sel-conflict-...`) with one expected skill needs a written
  tie-break: one of the rejected skills says `Not for ... (use <expected>)`. When
  no description settles it, the case has to be declared a tie, and the check
  lists it.
- Two skills whose trigger clauses share a lot of wording need a description
  that names the other, or a prompt that expects one and rejects the other.
- No skill name is the start of another, so `Skill: <name>` answers stay
  unambiguous.

## What it does not prove

It does not show that a model picks the expected skill. The overlap measure is
shared words in the `Use when` clause, which misses synonyms and flags pairs
that share only common words, so it is used to find pairs that need a prompt,
never to score routing. A passing check means the corpus covers the descriptions
and every known conflict is written down. Whether the descriptions work is a
question for the model run.

The model run is also a proxy. It gives the model the whole list as text and
asks for one name. A real host also decides whether to use a skill at all, and
Claude tends to skip skills for simple one-step requests. A core install sees 23
skills, not the full list. Use the run to compare descriptions against each
other, not as a measure of how often a skill fires in real sessions.

## Run the prompts through a model

```sh
python3 -m evals.behavior.runner --suite selection --model sonnet \
  --report /tmp/mastermind-selection.json
```

Each case passes when the answer contains the expected `Skill: <name>` and none
of the rejected ones. Run a case several times before trusting one result.

## Add a prompt

1. Write what a person would type. Do not name the skill.
2. Expect one skill, or list two in `contains_any` when both are right.
3. Reject the nearest skill a host could pick by mistake. Prompts that share
   words with the expected skill but need something else are the most useful.
4. Start the id with `sel-conflict-` when two triggers both fit. Then either
   write the tie-break into the losing skill's description or list both as
   expected.
5. Run `python -m evals.selection --verbose` to see trigger overlaps and the
   declared ties.

## Current findings

Four ties are declared because no description settles them:

| Prompt | Both fit |
|---|---|
| Run the tests that matter for my change | `mastermind-test-impact`, `mastermind-verify-change` |
| Sanity-check a plan to move emails onto a queue | `mastermind-architecture-review`, `mastermind-critical-review` |
| Review a PR that changes how tokens are stored | `mastermind-diff-review`, `mastermind-agent-security-review` |
| Review a PR that refactors a React page | `mastermind-diff-review`, `mastermind-frontend-audit` |

Three core skills hand off to a skill the core profile does not install:
`mastermind-agent-handoff` to `mastermind-prompt-refiner`, and
`mastermind-component-research` and `mastermind-product-intake` to
`mastermind-design-intake`. The check reports these as notes.

## Sources

- Claude Code, [subagents](https://code.claude.com/docs/en/sub-agents): Claude
  delegates when a task matches a subagent's description, descriptions should
  stay short, and "use proactively" encourages delegation.
- Codex, [skills](https://developers.openai.com/codex/skills): implicit use
  depends on the description, the key use case goes first because long lists
  are shortened, and prompts should be tested against the description.
- Codex, [subagents](https://developers.openai.com/codex/subagents): a custom
  agent's description guides when Codex uses it, and Codex delegates when asked
  or when `AGENTS.md` or a skill tells it to.
- Anthropic, [skill-creator](https://github.com/anthropics/skills/blob/main/skills/skill-creator/SKILL.md):
  trigger evals mix prompts that should and should not fire, the useful
  negatives are near misses, and each prompt is run several times against a
  held-out split.
