---
name: mastermind-style-deep
description: Write a grounded portrait of how the author actually develops — design approach, code shape, comments, tests, optimization habits, what they pay attention to, and commit voice — into the "Design patterns & tendencies" section of ~/.mastermind/style.md. The structural signature the deterministic miner can't measure. Use when the user wants a real "write like me" profile, says "deep style", "design patterns", "qualitative profile", or notices `mastermind miner profile` only produced formatter-level rules.
metadata:
  version: 0.2.1
  authors:
    - mastermind
  tags:
    - workflow
    - style
    - miner
---

# Mastermind — deep style portrait

`mastermind miner profile` measures only lexical idioms (indentation, quotes, braces, line length) — what a formatter already normalizes, so it's near-zero signal for "write like me". This skill writes the part that matters: a grounded **portrait of how the author develops**. The binary gathers evidence; you (the agent, already running a model) read code and write the portrait. No `claude -p`, no separate auth.

## When to use

- The user wants a real "write like me" profile, not formatter-config rules.
- The user says "deep style", "design patterns", "qualitative profile", "make it richer".
- After `mastermind miner profile`, to add the section the deterministic core can't.

## What you're producing

A portrait of **how this person works** — the kind a senior writes after reading someone's PRs for a month. Organized by the dimensions below, every claim tied to a concrete tell. Prose per dimension, not a list of isolated counts — the measured static rules already live in the section above this one.

## Gather evidence — quantitative AND qualitative

Don't just grep. A portrait needs both numbers and read code:

1. **Counts with their contrast.** Any "prefers X over Y" needs BOTH counted — early-`return`/`let-else` vs nesting depth; iterator chains vs `for` loops; typed errors vs `Box<dyn Error>`; table-driven tests vs one-assertion-per-fn. A bare count of X is not evidence of a preference.
2. **Read 4–6 real files.** A core module (design), a hot path (optimization), a public API (ergonomics), a test file (test style), a recent diff. Greps can't see *why* or *what they watch for* — reading can.
3. **Commits.** `git log --author="$(git config user.name)" --no-merges --pretty=%s -100` for voice and granularity; open a few bodies.
4. **Enforcing config FIRST.** `rustfmt.toml` / `.eslintrc` / `pyproject` lint config, `#![deny(...)]` / `#![warn(...)]`, `clippy.toml`, CI lint steps. Anything a formatter or linter *forces* is not personal style — exclude it, or mark it "enforced". Do not credit a `///` on every fn as a habit if `#![deny(missing_docs)]` mandates it.
5. **Optimization signals.** Benchmarks (`criterion` / `#[bench]` / `*.bench.*`), `#[inline]`, `with_capacity`, caching/memoization, `tracing`/profiling spans, comments mentioning perf. Their presence — or absence — tells you whether they optimize and whether it's measured or by feel.
6. **Observability & safety signals.** Logging/tracing/metrics density, assertions, input validation, `#[must_use]`, where error boundaries sit.

## Dimensions (cover only where evidence supports; omit the rest)

- **Design & problem-solving** — what they reach for: error-handling philosophy, how they decompose, concurrency/ownership model, abstraction level, state management.
- **Code shape & organization** — module/file/function layout, granularity, naming tendencies. Exclude anything the formatter enforces.
- **Comments** — do they write them *at all*? Where, and what kind — why-comments, doc contracts, or none? Is the density lint-forced?
- **Tests** — parametrized/table-driven or one-off per case? Unit vs integration? Assert-heavy vs property-based? What do they actually cover — happy path, edges, error paths?
- **Optimization & performance** — do they optimize? Premature, or measured (benchmarks/profiling present)? What — allocations, hot paths, caching? Or do they trade perf for clarity?
- **What they pay attention to** — the throughline. Where the care and ceremony cluster: correctness, type-safety, error boundaries, observability, API ergonomics, backward-compat, security. Infer it; name it.
- **Commits** — voice (terse/verbose, imperative, conventional/ticket/bare), granularity (one concern or bundled), subject vs body.

## Hard rules

- **Ground every claim in a tell** — a count or a named example. `Optimizes only after measuring (3 criterion benches; no #[inline] in the hot loop)`, not `cares about performance`.
- **Count the contrast.** "X over Y" needs both numbers.
- **Exclude tool-enforced traits.** Check fmt/lint config first; a forced trait is a measured rule, not a signature.
- **Negative space is signal.** What they *don't* do — no property tests, no premature optimization, sparse comments — belongs in the portrait.
- **No generic praise.** `clean`, `readable`, `idiomatic`, `well-structured`, `best practices` — banned. If you can't tie it to a tell, cut it.
- **Write it as a portrait** — a short grounded paragraph per dimension, not isolated bullets. It should read like a description of a person, not a lint report.

## Inject into `~/.mastermind/style.md`

The portrait is the `## Design patterns & tendencies (interpreted)` section in the managed block, right before the `\n---\nThe planner reads this …` footer. Replace it if present; otherwise insert before the footer.

## Caveat — managed block is regenerated

This section lives in the managed block, so a plain `mastermind miner profile` re-mine overwrites it. This skill is how it gets rewritten — re-run it after a re-mine, or move the section into the manual block above to pin it.
