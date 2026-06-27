---
name: mastermind-style-deep
description: Write the LLM-interpreted "Design patterns & tendencies" section of the author's ~/.mastermind/style.md — the structural signature (error handling, control flow, decomposition, tests, commit voice) that the deterministic miner can't measure. Use when the user wants a richer "write like me" profile, says "deep style", "design patterns", or notices `mastermind miner profile` only produced formatter-level rules.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - workflow
    - style
    - miner
---

# Mastermind — deep style synthesis

`mastermind miner profile` measures only lexical idioms (indentation, quotes, braces, line length) — exactly what a formatter already normalizes, so it's near-zero signal for "write like me". The valuable part — how the author *structures* code — is semantic and needs a model. That's this skill's job: the binary gathers evidence, **you** (the agent, with a working model already) write the prose. No `claude -p` subprocess, no separate auth.

## When to use

- The user wants a real "write like me" profile, not the formatter-config rules.
- After `mastermind miner profile` — to add the section the deterministic core can't.
- The user says "deep style", "design patterns", "make the profile richer".

## Steps

1. **Refresh the deterministic rules.** Run `mastermind miner profile` in the target repo (add `--force` only to reset the cross-repo store first). This writes the measured rules into `~/.mastermind/style.md`. Note the resolved author it reports (`enriched as <author>`).

2. **Gather evidence — quantified, not eyeballed.** Sample the author's contribution and *count* structural signals with patterns appropriate to the repo's language(s). For a Rust repo that's `grep -c` over the tree for `?`, `Box<dyn …Error>`, `.unwrap()`/`.expect()`, `let … else`, early `return`, iterator chains, `#[test]`, `///`; for TS/JS it's `try/catch`, `?.`, `.then` vs `await`, `.map/.filter`, `describe/it`, JSDoc; adapt. Also pull commit subjects: `git log --author="$(git config user.name)" --no-merges --pretty=%s -100`. Every claim you make must point to a number.

3. **Write the section** titled exactly `## Design patterns & tendencies (interpreted)`. Cover only what the evidence supports, across: error handling, control flow (early-return vs nesting), decomposition (function size, public surface, helper naming), comment/doc habits, test structure, and commit voice.

4. **Inject it into `~/.mastermind/style.md`.** The section lives in the managed block (between `<!-- mastermind-style:managed:start -->` and `:managed:end`), right before the `\n---\nThe planner reads this …` footer. If a `## Design patterns & tendencies (interpreted)` section is already there, replace it; otherwise insert before the footer.

## Hard rules for the section

- **Ground every claim in a concrete tell with the count.** `Propagates errors with \`?\` (436 sites) …`, not `handles errors well`.
- **Be falsifiable and specific.** No generic praise — `clean`, `readable`, `best practices`, `well-structured` are banned.
- **At most 8 bullets.** Each: the tendency, then the concrete tell.
- A tendency the repo's formatter/linter already enforces does **not** belong here — that's a measured rule, not a structural signature.

## Caveat — managed block is regenerated

This section sits in the managed block, so a plain `mastermind miner profile` re-mine overwrites it. This skill is how it gets regenerated — re-run it after a re-mine, or move the section into the manual block above if the author wants it pinned.

## Example bullet

```
- **Guard-clause early returns over nested `if`.** 177 early `return`s and 43 `let … else {`
  (`let Some(x) = … else { return }`) — the unhappy path exits at the top, the happy path
  stays unindented.
```
