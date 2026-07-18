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

# Codegraph research — discover structure with mmcg

The shared structural discovery layer for Mastermind. Claims about symbol
existence, indexed callers, imports, and bounded blast radius come from a fresh
mmcg result rather than memory. Exact source contracts and runtime behavior come
from source reads and tests.

The graph is syntactic evidence: name resolution, dynamic dispatch, reflection,
generated code, re-exports, and cross-language edges can reduce precision.

**Never name a symbol, file, caller, or blast radius from memory.** "I think `X` exists" is not evidence; `mmcg_search X` returning a hit is. A spec, audit, or critique built on a guessed symbol fails at the first step that touches real code.

## Structural vs literal

- **Structural discovery** (symbols, indexed callers/callees, imports, bounded blast radius) → mmcg first. It understands syntax better than literal text search, but remains name-based and bounded.
- **Literal** (string contents, log messages, comments, config values) → `Grep` / `Read`. mmcg doesn't index strings.
- **Runtime contract** (dynamic dispatch, reflection, generated code, re-exports, cross-language edges, exact branch behavior) → read source and run focused tests.

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

**mmcg-first:** use the graph to find candidate symbols and impact, then read the
source needed for the decision. Re-check with literal search or another source
when collisions/precision warnings are present, a security-sensitive path is at
stake, a meaningful zero-result could change the decision, or the language
feature is outside the graph's precision envelope. Do not repeat equivalent
searches when the indexed result already answers a low-risk discovery question.

## Stale or unavailable index

- No `mmcg_status` response → mmcg is unavailable. For a low-risk task, proceed
  with source inspection and state the limitation. Stop for user/planner review
  only when the missing structural evidence is load-bearing to scope or safety.
- `mmcg_status` reports stale files → re-index (`mastermind watch`, or a fresh index) before trusting structural answers. A stale graph is worse than none: it looks authoritative and is wrong.

## Citations

Whenever you read code or report a structural fact, carry the `file:line`. Downstream agents (planner, auditor) need precise locations to act — a finding without a citation is unverifiable and gets treated as a guess.

## Don't guess

Catch yourself guessing a signature, a path, or a caller count → query the graph
or read the source. Preserve `stale`, collision, precision, and truncation
metadata with any downstream claim.

## Related skills

- [[mastermind-structured-report-contract]] — the executor/auditor report tail you produce or consume
- [[mastermind-investigation-ledger]] — diagnose an unknown bug before drafting a spec
