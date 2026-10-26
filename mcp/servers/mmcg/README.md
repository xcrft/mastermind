---
name: mmcg
description: Mastermind Codegraph — local multi-language code indexer for Python, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust, C#, Go, Java, PHP, and C/C++. Stores symbols, calls, imports, evidence, and project history in SQLite and exposes bounded MCP tools.
metadata:
  version: 3.4.2
  authors:
    - mastermind
  tags:
    - mmcg
    - codegraph
    - python
    - typescript
    - javascript
    - vue
    - rust
    - csharp
    - go
    - java
    - php
    - cpp
  transport: stdio
  source: this repository
---

# mmcg — Mastermind Codegraph

mmcg indexes repository symbols, calls, imports and project evidence into a
local SQLite database. Use the CLI, MCP server or Lens UI to inspect structure,
change impact and verification evidence.

## Install

With Cargo (Rust 1.96+):

```bash
cargo install mmcg --locked
mmcg --version
```

Or install a prebuilt binary with Node.js 24+:

```bash
npm install -g @xcraftmind/mastermind
mastermind --version
```

The npm package provides both `mastermind` and `mmcg`. Cargo provides `mmcg`.
SQLite and tree-sitter are bundled.

## Index and review

Run inside a Git repository. Replace `main` with the baseline you want to review.

```bash
mmcg index .
mmcg map .
mmcg impact --since main
mmcg ui --since main
```

The default index is `.mastermind/mmcg.db`. Indexing is incremental. Use
`mmcg watch` while editing or `mmcg index . --force` to reparse all files.

| Task | Command |
|---|---|
| Look up a definition | `mmcg query search PaymentService` |
| Find containing callers | `mmcg query callers charge` |
| Find symbols by concept | `mmcg concept "payment retry handler" --top 10` |
| Review operation relevance against source | `mmcg concept-review prepare "payment retry handler" --top 5` then `mmcg concept-review finish --packet packet.json --assessment assessment.json` |
| Build a question-specific shortlist | `mmcg repo-map "payment retry handler" --source-sha256 <verified-source-digest> --budget-bytes 1600` |
| Prepare a role briefing | `mmcg brief --role executor --since main --budget-tokens 2000` |
| Search project history | `mmcg history "retry policy"` |
| Add compiler-resolved evidence | `mmcg enrich --scip index.scip` |
| Add declarative facts | `mmcg enrich --facts facts.json` |

Supported languages: Python and type stubs, TypeScript/TSX, JavaScript/JSX,
Vue SFC, Rust, C#, Go, Java, PHP and C/C++.

Concept search returns schema-v5 retrieval candidates with Porter stemming.
Terms with the same stem count once. Full AND matches take precedence. Queries
with at least three distinct terms also admit partial candidates matching at
least half the terms, rounded up, with a minimum of two, plus
`min(3, overall floor, ranking_terms count)` informative terms. Partial candidates
can fill spare page positions after full matches. The `all_and_partial_terms`
mode reports a qualified corpus containing both groups; `indexed_total` counts
their union before the page limit. Ranking prioritizes informative-term
coverage, then total coverage and BM25, so full matches remain first. English
function words keep their overall admission conditions and have no primary
ranking weight when other terms remain. Responses disclose `query_terms`,
`ranking_terms`, `match_mode`, `minimum_matched_terms`, `minimum_ranking_terms`,
`include_tests` and each candidate's `matched_terms`. Recognized Rust test
scopes are excluded before counting and limiting by default: parsed `test`,
`rstest`, or namespaced `::test` attributes, plus descendants of canonical
`#[cfg(test)]` modules. Use `mmcg concept "retry handler" --include-tests` or MCP
`include_tests: true` to restore all otherwise qualifying rows. The cfg-module
check does not recognize compound expressions, comments within the canonical
prefix or additional attributes. Unmarked scopes and other languages remain
unclassified. The reference below
describes the bounded canonical-module check.
For operation relevance, call `mmcg_concept_prepare`, inspect the cited source,
then submit a complete assessment to `mmcg_concept_finish`. The CLI equivalents
are `concept-review prepare` and `concept-review finish`. Preparation preserves
up to 50 lexical and 64 operation-body candidates, then adds up to sixteen
matching functions/methods from each of two nominated file owners through
`owned_scope`. Omissions and caps stay explicit.

Before retrieval, freeze a query-only `request_contract`: the requested
`operation` and exact quoted `constraints` classified as input, state, scope or
result. Pass it to prepare (CLI `--request-contract PATH`). Each relevant
assessment needs a typed `witness` describing its own behavior and matched,
source-bound evidence for every declared constraint. Select roles after source
inspection. A generic prerequisite is context; a handler or module can qualify
through its own operation-specific branch. An omitted selector cannot stand in
for absent runtime state. The original query remains authoritative, including
constraints absent from a default empty ledger.

The calling agent follows `operation-roles-v3`: relevant entries declare
`implementation`, `entrypoint`, `contract`, `essential_helper` or
`supporting_context`, with own-declaration citations. All non-implementation
roles need inspected, source-bound `supporting_evidence` for their caller,
consumer, implementation or navigation path; entrypoints/contracts/helpers
need evidence outside their own declaration. Unresolved original intent goes
in `query_ambiguities`, restricting useful results to supporting context.
Context needs specific inspected navigation to an existing requested operation
satisfying every explicit constraint. Opposite or missing behavior, narrower
scope and locations for adding a feature are irrelevant; uninspected constraints
remain unknown. Prefer semantic roles for specific result entrypoints and
necessary domain steps, including inspected type/consumer paths.
Uninspected entries stay unknown. The full `candidates` page retains useful
navigation; `semantic_candidates` and `supporting_context` partition that page.
Useful lexical entries keep their order before source supplements. These are
client source judgments; protocol checks do not establish semantic truth.
The `mmcg-concepts-v5` corpus
contract refreshes managed indexes once when needed. Custom indexes remain
read-only; rebuild an older custom index with
`mmcg --index /path/to/index.db index /path/to/repository --force` before retrying.
See the [concept search contract](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md#local-symbol-concept-search)
for limits, ordering and coverage semantics.

`repo-map` returns a JSON navigation payload using `focused_shortlist_v3`: weighted
task-term overlap, grouped file locations, at most three files and three definitions
per file. It leaves spare budget unused and excludes declared test functions. It
is an experimental query heuristic, not PageRank or source evidence. Read original
source at the selected locations and follow the relevant call paths.

Before requesting a map, validate the source projection, index freshness and
allowlist, then provide that projection's SHA-256 as `--source-sha256`. The digest
is supplied by the caller; this command does not verify an arbitrary digest against
live files. It opens the index read-only, checks schema compatibility and rejects
candidate sets larger than 20,000 instead of silently ranking a partial prefix.
Invalid digest/budget/location or an index change exits unsuccessfully. Rebuild a
stale index against the intended scope and generate a new binding before retrying.
The repository report `evals/baselines/repo-map-20261012.md` records resource use
and provisional model reviews on eight questions; human reference is pending.

For component comparisons, use `--payload map-only` or
`--payload instructions-only`; the default is `bundle`. All variants use the
same selected locations and combined-header byte reservation, so removing advice
does not introduce additional definitions. `payload_bytes` charges the text
actually returned; instruction-only output has zero selected locations.

Use `--ranking graph` to evaluate experimental personalized PageRank; the default
is `--ranking lexical`. Both modes retain the same advice, byte cap and shortlist
limits. Graph mode uses lexical scores as seeds and deduplicated, bidirectional
`calls`/`references` links between eligible definitions. Targets must have a unique
same-language name and compatible kind. Links are navigation hints, not resolved
semantic evidence.

The ranker runs 24 iterations with damping 0.85; dangling rank returns to the seed
distribution. Stable source locations break ties. Graph collection probes a
100,000-edge cap and discards overflowed graphs. Empty graphs, absent seeds and
overflow fall back to lexical selection; the JSON `graph` field records the
fallback, scanned/omitted counts and usable links. SQLite query errors still fail
the command. Keep graph ranking opt-in; require a new quality and cost
qualification before enabling it in an agent workflow.

## Connect an agent

```bash
mmcg setup claude --scope user          # preview
mmcg setup claude --scope user --write  # apply
```

For direct stdio use, run `mmcg serve`. Structural MCP queries may refresh the
managed index before reading it. Custom indexes require an explicit refresh.
Other tools are read-only except local scratchpad writes and task-mining submissions.

See [client integrations](https://github.com/xcrft/mastermind/tree/main/docs/integrations)
for Claude Code, Codex, Cursor, Continue and generic MCP clients.

## Interpret results

| Result | Evidence and limits |
|---|---|
| Default graph | Syntactic, with gaps for name collisions, dynamic dispatch, reflection, generated code and cross-language calls |
| SCIP | Separately identified compiler evidence |
| Imported facts | Source and revision metadata, no executable plugin loading |
| Unreferenced symbols | Candidates for review, no proof of runtime unreachability |
| Candidate tests | Suggested coverage, no observed test result |
| Stale inputs, omissions and work limits | Reported in the result, source-current evidence still needs interpretation |

Indexing, queries and Lens run locally. Explicit model-backed commands such as
native execution, review or persona analysis use the selected client/provider.
See the workflow and persona guides before enabling them.

## Reference and development

- [CLI, MCP tools, limits and precision](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md)
- [Task workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md)
- [Persona capture and review](https://github.com/xcrft/mastermind/blob/main/docs/guides/persona-hooks.md)
- [Fact-ingestion contract](https://github.com/xcrft/mastermind/blob/main/docs/fact-ingestion-sdk.md)
- [Contributing and validation](https://github.com/xcrft/mastermind/blob/main/CONTRIBUTING.md)
- [Benchmark methodology](https://github.com/xcrft/mastermind/blob/main/docs/benchmarks.md)

License: [MIT](https://github.com/xcrft/mastermind/blob/main/LICENSE).
