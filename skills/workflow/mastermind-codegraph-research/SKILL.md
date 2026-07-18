---
name: mastermind-codegraph-research
description: Use mmcg to ground structural code claims before planning, auditing, criticizing, or researching code. Triggers when an agent needs symbol existence, callers, callees, imports, blast radius, file existence, or stale-index handling.
metadata:
  version: 0.1.1
  authors:
    - mastermind
  tags:
    - workflow
    - mmcg
    - codegraph
---

# Codegraph research — ground structural claims in mmcg

The shared truth layer for every Mastermind subagent. Any claim about code *structure* — does a symbol exist, who calls it, what it imports, how big a change is — comes from the mmcg codegraph, not from memory.

**Never name a symbol, file, caller, or blast radius from memory.** "I think `X` exists" is not evidence; `mmcg_search X` returning a hit is. A spec, audit, or critique built on a guessed symbol fails at the first step that touches real code.

## Structural vs literal

- **Structural** (symbols, callers, callees, imports, dependencies, blast radius) → mmcg. Faster, cheaper, and more accurate than grep for code structure.
- **Literal** (string contents, log messages, comments, config values) → `Grep` / `Read`. mmcg doesn't index strings.

## Query decision table

| Question | Tool |
|---|---|
| Does symbol `X` exist? (get `file:line` + signature) | `mmcg_search` |
| What calls `X`? | `mmcg_callers` |
| What does `X` call? | `mmcg_callees` |
| If I change/rename `X`, what breaks? (transitive) | `mmcg_impact` |
| What does file Y import? | `mmcg_imports` |
| Who imports `X` / this path? | `mmcg_imported_by` |
| Does this file path exist in the index? | `mmcg_files` |
| Is the index ready / how stale is it? | `mmcg_status` |
| String contents / comments / log lines | `Grep` |
| File-name / extension globs | `Glob` |

**mmcg-first:** for any who/what/where question about code, try the mmcg tool first. Fall back to `Grep` / `Read` only when mmcg returns nothing or the question is non-structural. Do NOT re-verify mmcg results with grep — that wastes context.

## Stale or unavailable index

- No `mmcg_status` response → mmcg isn't configured. Say so to the user; ask whether to proceed without truth grounding or wait until the index is set up. Don't silently work blind.
- `mmcg_status` reports stale files → re-index (`mastermind watch`, or a fresh index) before trusting structural answers. A stale graph is worse than none: it looks authoritative and is wrong.

## Citations

Whenever you read code or report a structural fact, carry the `file:line`. Downstream agents (planner, auditor) need precise locations to act — a finding without a citation is unverifiable and gets treated as a guess.

## Don't guess

Catch yourself guessing a signature, a path, or a caller count → stop and call mmcg. The two-second query is always cheaper than the failed executor cycle a wrong guess causes.

## Related skills

- [[mastermind-structured-report-contract]] — the executor/auditor report tail you produce or consume
- [[mastermind-investigation-ledger]] — diagnose an unknown bug before drafting a spec
