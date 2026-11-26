# Skill selection

A host picks a skill or subagent from its name and description alone, before
it reads the body. Two tools look at that choice:

- `python -m evals.selection` checks the case list and the catalogue. It runs
  without a model and is part of `just check`. It never decides which choice a
  prompt should get, so a green run says nothing about routing quality.
- The `selection` suite in [behavioural evals](../behavior/README.md) sends the
  prompts to Claude. That is the only thing here that measures picks.

The cases live in [`../behavior/cases/selection.jsonl`](../behavior/cases/selection.jsonl).
[`catalogue.md`](catalogue.md) is the system prompt for model runs. It lists
every skill and every subagent, because Claude Code sees both. It is built from
the descriptions, so run `python -m evals.selection --write` after you change
one, or the check fails.

## A case

```json
{"id": "sel-conflict-critical-over-diff",
 "why": "...",
 "input": {"prompt": "Review my design for splitting the worker pool. No code is written yet."},
 "expect": {"contains_any": [["Skill: mastermind-critical-review", "Agent: mastermind-critic"]],
            "not_contains": ["Skill: mastermind-diff-review"]},
 "tie_break": "a design with no code yet"}
```

- `contains` or `contains_any` holds the right answers. A subagent that does a
  skill's job (`COMPANIONS` in `__init__.py`) is accepted with it, because
  Claude Code may delegate instead of loading the skill. That pair is one
  choice, not a tie.
- `not_contains` lists the near misses. The model pass rule ignores it, but the
  check uses it.
- `tie_break` is required on a `sel-conflict-` case. See below.
- `lexical_exception` is a reviewed reason for a case whose words point away
  from its label. See below.
- `why`, `tie_break` and `lexical_exception` never reach the model. Only
  `input.prompt` does.

## What the deterministic check does

| Check | Fails when |
|---|---|
| Shape | a name is not a shipped skill or subagent, a case rejects nothing, or a choice is both expected and rejected |
| Name leak | the prompt names an expected choice, in any case, with or without `mastermind-`, with hyphens or spaces |
| Nonsense | the prompt has fewer than 5 words or no function word, like "asdf qwer" |
| Lexical plausibility | the expected choice does not score above every rejected one on shared description words, and no `lexical_exception` explains why. A `none` case fails when some choice shares too many words with it. An exception the words do not need also fails |
| Conflicts | a `sel-conflict-` case has no `tie_break`, the cited text is not in a rejected choice's `Not for ... (use <winner>)` clause or only in the winner's trigger, the winner sends that text back to the loser, or the prompt matches the winner's hand-off to the loser as well as the cited clause |
| Known neighbours | a pair in `ADJACENT_PAIRS` has no conflict case that expects one and rejects the other |
| Trigger overlap | two `Use when` clauses share 20% or more of their words and no case expects one and rejects the other. A link between the descriptions no longer excuses it |
| Hand-offs | a core skill hands off to a skill core does not install, or three or more skills hand a prompt on in a loop |
| Coverage | a core skill has fewer than two cases for it or two against it, or another skill fewer than one |
| Leak rate | more than one third of positive cases, in the corpus or in the held-out split, share two or more rare description words only with the expected choice, or a `-paraphrase` case shares any rare word with it |
| Split | the held-out split is outside 30% to 50% of the cases |
| Catalogue | `catalogue.md` is out of date |

### The lexical gate and what it cannot catch

Each choice gets the words of its name and description, without its `Not for`
hand-offs, weighted by how rare they are across all choices. The expected
choice must score higher than every rejected one. This catches a label moved
to the wrong skill, a prompt pasted from another case and gibberish.

It cannot catch:

- a wrong label on a case with a `lexical_exception`. The exception is a
  claim a reviewer has to read. 25 of the 91 cases have one, mostly
  paraphrases and the meaning-based cases that make the corpus worth having
- a wrong label whose prompt happens to share more rare words with the wrong
  skill than with the rejected ones
- a wrong choice among skills the case does not reject
- synonyms. "Hex colours" and "raw values" score nothing in common

### Conflict cases

A conflict case is a prompt where two triggers both fit. `tie_break` quotes
the text that settles it in one direction:

1. a phrase from a rejected choice's `Not for ... (use <winner>)` clause, or
2. a phrase only the winner's trigger claims, for a pair where the loser
   cannot name the winner. A core skill cannot hand off to a skill core does
   not install, so product intake does not name design intake, and design
   intake claims "even with a ticket" instead.

Swapping the winner and the loser of any conflict case fails the check. When
two descriptions name each other, the prompt has to share more words with the
cited clause than with the winner's own hand-off to the loser, so the case
says which side of the boundary it is on.

### Lexical overlap is only a hint

Shared trigger words find pairs that need a case. They miss pairs that mean
the same with other words, like the ledger and verify-change, which share no
trigger words. Those pairs are in `ADJACENT_PAIRS`, which is maintained by
hand. Add a pair there when a person could confuse two skills.

## Train and held-out split

`selection_split()` in the runner hashes each case id and puts about 40% of
cases in a held-out split. Nobody picks which cases are held out, and a new
case never moves an old one. Change descriptions only in response to misses
in the train split. Read the held-out score to see whether a change carried
over to prompts you did not tune against.

## Run the prompts through a model

```sh
python3 -m evals.behavior.runner --suite selection --model sonnet --runs 3 \
  --report /tmp/mastermind-selection.json \
  --selection-summary /tmp/mastermind-selection-summary.json
```

A run passes only when the answer has exactly one `Skill: <name>` or
`Agent: <name>` line, after stripping backticks, quotes and spaces, and the
name is exactly one of the expected answers. With `--runs 3` each prompt runs
three times, as skill-creator suggests, and the case passes when most runs
pass. With three runs that is two, the same as skill-creator's 0.5 trigger
rate.
`--split train` or `--split held-out` runs one split. The summary prints the
accuracy per split and the misses for each expected choice, and
`--selection-summary` writes every run's pick as JSON.

The model run is still a proxy. It gives the model the whole list as text and
asks for one name. A real host also decides whether to use a skill at all, and
Claude tends to skip skills for simple one-step requests. A core install sees
23 skills, not the full list. Use the run to compare descriptions with each
other, not as a measure of how often a skill fires in real sessions.

## Add a case

1. Write what a person would type. Do not name the skill.
2. Expect one choice. Add its companion subagent if it has one. The check
   tells you when one is missing.
3. Reject the nearest choice a host could pick by mistake.
4. Start the id with `sel-conflict-` when two triggers both fit, and quote the
   settling clause in `tie_break`. If no description settles it, change the
   description of the skill that should give way.
5. If the check says the words do not support the label and you are sure the
   label is right, write a `lexical_exception` that says which words in the
   prompt map to which part of the trigger.
6. Run `python -m evals.selection --verbose` to see overlaps and the cases
   that share rare words with their label.

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
  negatives are near misses, each prompt runs three times, and a 60/40
  train and held-out split keeps description tuning honest.
