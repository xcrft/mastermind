---
name: mastermind-codegraph-research
description: Use mmcg before Bash or literal search for repository orientation, natural-language symbol discovery, symbol existence, callers, callees, imports, blast radius, file existence, or stale-index handling.
metadata:
  version: 0.3.0
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

- **Bounded orientation** (relevant changes, symbols, callers, tests, and history
  for a role) → one `mmcg_brief` before broad discovery.
- **Concept discovery** (the intent is known but the exact symbol is not) →
  `mmcg_concept`, then an exact structural query on the selected candidate.
- **Structural discovery** (symbols, indexed callers/callees, imports, bounded blast radius) → mmcg first. It understands syntax better than literal text search, but remains name-based and bounded.
- **Literal** (string contents, log messages, comments, config values) → `Grep` / `Read`. mmcg doesn't index strings.
- **Runtime contract** (dynamic dispatch, reflection, generated code, re-exports, cross-language edges, exact branch behavior) → read source and run focused tests.

## Query decision table

| Question | Tool |
|---|---|
| What bounded context does this planner/executor/auditor need? | `mmcg_brief` |
| Which local symbols match this natural-language concept? | `mmcg_concept` |
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
Do not use Bash to rediscover symbols, files, callers, imports, or impact from a
fresh, complete graph response. Bash remains appropriate for Git, builds,
tests, linters, logs, and runtime probes.

## Stale or unavailable index

- No mmcg response → mmcg is unavailable. For a low-risk task, proceed
  with source inspection and state the limitation. Stop for user/planner review
  only when the missing structural evidence is load-bearing to scope or safety.
- A structural query refreshes a stale managed index once before answering. If
  that bounded refresh fails, preserve `index_stale` or
  `refresh_limit_exceeded` and use a source fallback; do not retry in a loop.
- A custom external index is read-only and must be refreshed explicitly with
  `mastermind index`. Use `mmcg_status` only for diagnosis or after a warning,
  not as a mandatory first call.

## Citations

Whenever you read code or report a structural fact, carry the `file:line`. Downstream agents (planner, auditor) need precise locations to act — a finding without a citation is unverifiable and gets treated as a guess.

## Don't guess

Catch yourself guessing a signature, a path, or a caller count → query the graph
or read the source. Preserve `stale`, collision, precision, and truncation
metadata with any downstream claim.

## Epistemic envelope

For any conclusion that changes scope, risk, or implementation, separate:

- **Observed** — direct graph result, source line, test output, or runtime fact.
- **Inferred** — the conclusion drawn from those observations and why it follows.
- **Unknown** — missing evidence that could change the conclusion.
- **Confidence** — `high`, `medium`, or `low`, plus a concrete reason. Never derive
  confidence from the number of search hits alone.
- **Would change this conclusion** — the contradictory source, runtime result,
  collision resolution, or fresh index result that would falsify it.

Do not hide a speculative leap inside an observed-facts paragraph. A zero result
means "not found in this index under this query," not "does not exist."

## Related skills

- [[mastermind-structured-report-contract]] — the executor/auditor report tail you produce or consume
- [[mastermind-investigation-ledger]] — diagnose an unknown bug before drafting a spec
