# MCP server

`mastermind serve` runs the Mastermind MCP server over stdio. It exposes 37 tools
over the local index: 22 queries that may refresh the managed index first, 14
read-only operations and 1 additive write. To connect a client, use its guide in
[integrations](../integrations/generic-mcp.md). Indexing rules and CLI commands
are in the [CLI and index reference](mmcg.md).

## What the server can write

| Tool behavior | Write boundary |
|---|---|
| Structural queries | May refresh the managed derived index |
| `mmcg_mining_submit` | Stages source-cited proposals from the current task |
| Every other tool | Read-only |
| Arbitrary SQL or executable plugins | Not exposed |

Tool annotations are advisory, and returned tool content is untrusted data.
Neither grants a permission or skips a confirmation.

## Protocol contract

The server supports two lifecycles. A request whose `_meta` names a protocol
version, and every `server/discover` call, uses the per-request lifecycle.
Before `initialize`, any request with `_meta` takes that path too. Other
requests use `initialize`.

| Lifecycle | Revisions | Version choice |
|---|---|---|
| `initialize`, then `tools/list` and `tools/call` | `2025-11-25`, older `2024-11-05` | `2024-11-05` when requested, `2025-11-25` for any other request. A client that cannot use the answer disconnects |
| Per-request `_meta`, no `initialize` | `2026-07-28` | `server/discover`, `tools/list` and `tools/call` carry `io.modelcontextprotocol/protocolVersion` and `io.modelcontextprotocol/clientCapabilities` in `_meta`. Another version gets error `-32022` with `supported` and `requested` in its data. Missing or malformed metadata gets `-32602` |

`server/discover` lists only `2026-07-28`. Per-request results add
`resultType: "complete"` and the server info in `_meta`.

| Rule | Contract |
|---|---|
| Result shape | `2025-11-25` and `2026-07-28` results carry compact JSON text and an object `structuredContent`. `2024-11-05` results are content-only |
| Errors | Tool execution and input errors use `isError: true`. Malformed protocol requests and internal failures use JSON-RPC errors |
| Input frame | At most 1 MiB. A larger frame closes the connection |
| Response | The whole JSON-RPC response stays within 8 MiB, counting the duplicated text and structured content. A larger result asks the caller to narrow the query |
| Budget and cancel | Every call runs under a work budget and stops on a client cancel notification. See [work budgets](mmcg.md#work-budgets-timeouts-and-cancellation) |

## Managed index refresh

Structural tools, concept queries, review preparation and completion, and
briefs share one freshness check across CLI and MCP. Each managed operation
makes at most one refresh attempt under its original deadline. The server
derives the repository root from the canonical database path and checks it
against the stored index identity before any refresh.

| Condition | Result |
|---|---|
| Source files or extractor contract drifted | One refresh of `.mastermind/mmcg.db`, then the query runs |
| More than 100,000 supported source files or 512 MiB of source bytes | `refresh_limit_exceeded`, with no partial refresh |
| Changed extractor or concept contract | A full rebuild. If it does not fit the request budget, the call returns `index_rebuild_required`. Run `mastermind index .` or raise `MMCG_QUERY_BUDGET_MS` |
| Another writer holds the index | `index_busy` with `retryable: true`. Retry the same call |
| Failed or unavailable refresh | `index_stale`. The payload keeps its code and coverage fields, and `stale_reason` names `structural`, `concept` or `history` drift. History-only drift also sets `history_freshness: stale`, even with zero `stale_files` |
| Listed source that is not a regular file, such as a `.py` symlink | Never indexes and does not make the index stale. `mmcg_status` and `index_stale` list up to 20 such paths in `unindexable_sources`. A source that was indexed and then replaced by a symlink is stale until the refresh removes its old rows |
| Tracked deletion | Fresh only when the path is absent from both the worktree and the index for the whole check |
| Missing, non-regular or non-UTF-8 tracked paths | Make the inventory incomplete, so the index is stale |
| `mmcg_status` | Runs its freshness scan under the request budget |

A custom external `--index` opens read-only. The server never creates,
migrates, truncates or adds a WAL to it, so refresh it yourself with
`mastermind --index PATH index ROOT`. `serve` reads a private snapshot taken at
start. After `index` rewrites the external file, tools return `index_replaced`
until you restart the server. An incompatible custom schema returns
`schema_incompatible`. Reading an active WAL may create or update its SHM file.

Indexed source keys use one lossless repository-relative encoding with `/`
between components. A non-UTF-8 component, or a literal backslash in a Unix file
name, is rejected so it never takes another file's key. Other valid file name
bytes, including newlines, are kept.

## MCP tools

Tool responses are bounded JSON. Collection responses report their own
`total`, returned `count` and truncation. Status and workflow responses use named
fields. Where a tool says the CLI keeps a complete listing, the matching
`mastermind query` command has no MCP row cap.

### Read and reuse source ranges

`mmcg_read` takes a repository-relative `file`, a task or session label in
`task`, and optional inclusive `start_line` and `end_line`. Each reply carries
the file SHA-256, absolute-line `segments`, an opaque `receipt` and continuation
fields. It reads current bytes whatever the index freshness.

| Limit | Value |
|---|---|
| Default range | 80 lines |
| New lines per reply | 200 |
| File | UTF-8 regular file of at most 1 MiB. Hidden path components and symlinks are rejected |
| Ranges per receipt | 128 merged ranges. Past that, `receipt_coverage_truncated` is set and older ranges may come again |
| Receipts held | 128 in memory, never written to SQLite or shared across server processes |

| Request | Delivery |
|---|---|
| No `previous_receipt` | Requested text, with pagination |
| Matching receipt from the same task and file, text still in the caller's context | Only the missing ranges. `reused_ranges` names the omitted text |
| Changed file SHA, other task or file, expired receipt or restarted server | Requested text again, with `reuse_status` saying why |

Pass a receipt only while its text is still in the current task's context.
Drop it after a handoff or context compaction. When `range_truncated` is true,
continue at `next_line` with the original end.

`serve` binds reads to its working repository. For an external index, bind the
root explicitly, because index metadata alone does not allow file reads:

```sh
mastermind --index /absolute/index/mmcg.db serve --root /absolute/repository
```

### Effort selection

| Tool | Args | Returns |
|---|---|---|
| `mmcg_effort_route` | Optional `agent`, `task_family` (each at most 80 UTF-8 bytes), `read_only`, `bounded_scope`, `stable_inputs`, `lexical_repo_map`, `risk` (`low`, `high`, `unknown`), `user_effort` (`low`, `medium`, `high`, `xhigh`, `max`) | A task and agent policy, requested effort, reason, Codex and Claude argument arrays, and `codex_spawn_args` with `fork_turns: "none"` and `reasoning_effort` for Codex subagents. Missing context keeps high effort and an explicit human choice wins. `applied` and `quality_qualified` stay false |

The tool reads no source or index, calls no model and writes no configuration.
The controller applies the override to the next invocation, and the bundled
`mastermind-agent-handoff` skill applies it to subagents. The CLI takes the
same JSON through `mastermind effort-route --request route.json` or stdin, up to
16 KiB. See [effort routing](../guides/effort-routing.md).

### Symbol lookup

| Tool | Args | Returns |
|---|---|---|
| `mmcg_search` | `name` or `names` (1-8 distinct), optional `kind`, `language`, `collapse_partials` (default `true`), `top` (single: default 100, max 200. Batch: default 10, max 25 per name) | Exact-name candidates. A batch reads one snapshot and returns ordered `queries`, `query_count` and aggregate `truncated`. A database change during the batch fails it with `snapshot_changed`. With partial collapsing, at most 500 raw declarations per query are grouped. At that cap `total` is null and `truncation_reason` is `raw_work_limit`, so turn collapsing off for an exact raw total |
| `mmcg_callers` | `name`, optional `language`, `edge_kind` (default `calls`), `top` (default 100, max 500) | Containing functions that reference `name` by that edge kind. A function with three calls counts once. `edge_kind: imports` gives importers, `references` gives function-value and Rust macro-body usages. A reference is not proof of a call |
| `mmcg_callees` | `name`, optional `language`, `edge_kind` (default `calls`), `file`, `line` (needs `file`), `top` (default 100, max 500) | Outgoing names from one definition. `match_status` is `matched`, `ambiguous` or `not_found`. An ambiguous result has bounded `candidates` and no edges. `candidate_total` and `candidates_truncated` describe definition selection, `total` and `count` the edges. A `file` and `line` selector is queried on its own, so the candidate cap cannot hide it |
| `mmcg_impact` | `name`, optional `max_depth` (1-10, default 2), `language` | Transitive dependency candidates through `calls` and syntactic `references`, capped at 5,000 rows. The language filter applies to seeds and every step. Narrow `max_depth` or add a language when truncated |
| `mmcg_imports` | `file`, optional `top` (default 200, max 500) | Static imports declared by the file, each with `name`, the full `path` when extracted, and `line`. Dynamic imports are missing |
| `mmcg_imported_by` | `query`, optional `match` (`name` default, or `path`), `language`, `top` (default 200, max 500) | Files whose static imports reference the name or full path. Use `match: path` when a leaf name is ambiguous |
| `mmcg_symbols_in_file` | `file`, optional `top` (default 200, max 500) | Extracted symbols in source order |
| `mmcg_outline` | `file`, optional `top` (default 200, max 500) | Parent-preserving symbol tree. `top` bounds all nested nodes, and depth stops at 64 levels. `depth_limit` or `hierarchy_incomplete` means the tree is not a complete prefix |
| `mmcg_files` | Optional `prefix`, `language`, `top` (default 200, max 500) | Indexed files with symbol counts. Narrow by literal prefix or language when truncated |
| `mmcg_recent_changes` | `since` (for example `2h`, `30m`, `1d`), optional `top` (default 200, max 500) | Files whose stored mtime falls between `window_start_unix_ms` and `as_of_unix_ms`, future mtimes excluded. This is filesystem recency, not indexing time or Git history |
| `mmcg_change_class` | `file` | Whether the file's last change was `structural`, `cosmetic` or `first-seen`, from an FNV-1a 64-bit hash of its parsed shape without line numbers or whitespace. A large diff that is mostly cosmetic has a smaller real scope |
| `mmcg_unreferenced` | Optional `kind`, `language`, `top` (default 100, max 500) | Symbols with no indexed reference candidates, with per-symbol precision. Candidates to review by hand, see [Limitations](mmcg.md#heuristic-query-semantics) |
| `mmcg_api_surface` | `prefix`, optional `language`, `top` (default 100, max 500) | Symbols under the literal `prefix` with at least one outside reference candidate by name and kind. Same-named definitions can add false candidates, so this is empirical use, not declared visibility |
| `mmcg_concept` | `query`, optional `top` (default 10, max 50), `include_tests` (default false) | Symbol candidates from names, paths, declaration shapes and owned documentation, with matched terms, coverage and safety omissions from one snapshot. Scores are local to the query. See [concept search](concept-search.md#search-by-concept) |
| `mmcg_concept_prepare` | `query`, optional `top`, `include_tests`, `hints`, `request_contract` | A source-bound review packet: the lexical page plus body recall, declaration identities, source excerpts, an assessment template and coverage limits. See [operation review](concept-search.md#review-operation-relevance) |
| `mmcg_concept_finish` | Unchanged `packet`, complete `assessment` | Checks current source and the packet again, requires one judgment per candidate and citations for relevant entries, then orders the useful page. The calling agent owns the semantic judgment |

Every collection tool above reports exact `total`, returned `count`,
`truncated` and `row_limit`, echoes its effective filters and carries
`precision_notes`, including on empty results.

### Architecture and change analysis

| Tool | Args | Returns |
|---|---|---|
| `mmcg_map` | Optional `path` (default `.`), `depth` (1-6, default 2), `top` (1-100, default 20), `production_only` (default `false`) | Schema-v1 architecture briefing: entry points, components, boundaries, hotspots and import cycles for a file or directory scope. JSON, text, Mermaid and CLI SARIF are views of one result. See [map scope and caps](#map-scope-and-caps) |
| `mmcg_temporal` | `since`, optional `root`, `path` (default `.`), `depth` (1-5, default 2), `top` (1-100, default 20), `production_only`, `codeowners` | Schema-v1 delta between a Git baseline and the indexed worktree. See [temporal graph](mmcg.md#temporal-graph) |
| `mmcg_centrality` | Optional `prefix`, `language`, `kind`, `top` (default 20, max 200) | Symbols ranked by in-degree (distinct callers), without `<module>` rows and zero-degree symbols. Name-based resolution can pool collisions. Use the ranking as a reading-order hint |
| `mmcg_dependency_cycles` | Optional `language`, `min_size` (default 2), `top` (default 50, max 200) | Import cycles as strongly connected components of the file import graph. See [cycle limits](#dependency-cycle-limits) |
| `mmcg_symbols_changed_since` | `git_ref`, optional `root`, `top` (default 100, max 500) | Symbol diff between a Git ref and the index, reparsing old blobs with the same extractor. MCP returns at most `top` items from each of `files_in_diff`, `added`, `removed`, `signature_changed` and `errors`, and `coverage` keeps the exact totals. The file loop stops at 10,000. At that cap totals are null and `source_truncated` is set |
| `mmcg_change_impact` | `since`, optional `root`, `depth` (1-5), `top` (1-500) | Schema-v1 analysis of the baseline against staged, unstaged and untracked content: added, removed, signature- and body-changed symbols, dependency candidates through calls and references, component crossings, ranked test candidates, and a `disciplines` block. Non-UTF-8 paths are omitted, counted and make the file collection partial. Root, index freshness, Git snapshot and SQLite snapshot checks fail with stable codes |
| `mmcg_test_impact` | `since`, optional `root`, `depth` (1-5), `top` (1-500) | The test part of `mmcg_change_impact`. See [test candidates](#test-candidates) |
| `mmcg_brief` | `role` (`planner`, `executor` or `auditor`), `since`, optional `root`, `budget_tokens` (256-8,000, default 2,000) | One schema-v1 role packet. See [bounded role briefs](#bounded-role-briefs) |
| `mmcg_status` | none | Index path, file and symbol counts, contract flags, history freshness and source freshness from one snapshot. See [status fields](#status-fields) |

#### Map scope and caps

| Rule | Contract |
|---|---|
| Path scope | Lexical. `%` and `_` are literal bytes. Components under a selected directory are relative to it, root components stay repository-relative, and a selected file keeps its path |
| `production_only` | Leaves out conventional test, fixture, example, generated and vendor path segments and test file names (`test_*`, `*_test.*`, `*_tests.*`, `*.test.*`, `*.tests.*`, `*.spec.*`, `*Test.*`, `*Tests.*`) before the queries run |
| Hotspots | Unambiguous definitions come before pooled same-name collisions |
| Text and Mermaid | Text reports coverage per section and the returned cycle members. Mermaid shows entry points, components, boundaries, hotspots and cycle rings, and names the omitted counts when the picture drops rows |
| SARIF | Exports returned cycles. Its partial state comes from every top-level section and component boundary |
| Freshness | The briefing does not check source freshness |

| Cap | Value |
|---|---|
| Aggregation paths | 50,000 |
| Languages, components | 20 each |
| Boundaries | 20 per component, 400 in total |
| Entry points | 50 |
| Hotspots | 100 |
| Scoped cycle edges | 50,000 |
| Cycles, cycle memberships | 50, 500 |

| Truncation cause | Meaning |
|---|---|
| `path_work_limit` | Path-derived aggregates are partial |
| `language_limit`, `entry_point_limit`, `top_limit` | That section reached its cap |
| `top_probe` | A hotspot or per-component boundary probe found one more row than the cap |
| `global_probe_limit` | The 401st global boundary row left a component result uncertain |
| `cycle_limit`, `cycle_membership_limit`, `cycle_and_membership_limit` | Complete cycle output was bounded |
| Cycle `work_limit` | Cycle analysis was skipped, so no cycles are returned |

#### Dependency-cycle limits

| Bound | Result |
|---|---|
| More than 50,000 distinct import file-pair edges | Cycle analysis is skipped. `cycles` is empty, totals are null and `truncation_reason` is `graph_work_limit` |
| Complete cycle output | Capped by `top` and 500 file memberships in total |
| Membership cap reached | `member_limit`. A cycle that does not fit is omitted, never returned in part. `total` and `total_members` stay exact |

Imports resolve by leaf name within one language family. JavaScript, TypeScript,
TSX and Vue form one family, and every other language is its own. A same-named
symbol in another file of the family can add an edge, and a partial graph can
hide or split cycles. Check a cycle before you refactor, narrow by `language`,
or use the complete `mastermind query dependency-cycles`.

#### Test candidates

| Class | Evidence |
|---|---|
| Direct static | Changed tests and depth-1 graph tests |
| Transitive | Deeper graph tests |
| Heuristic | Same-component candidates without graph evidence in this response. `same_component_test_filename` for test-like paths, `same_component_test_attribute` otherwise |

Explicit test attributes identify candidates whatever the file name, including
inline Rust `test`, `tokio::test` and `async_std::test`. Name heuristics still
need test-like paths, and fixtures and lifecycle hooks stay excluded. The
classes rank candidates. They do not show that a test ran, what it covers or that
it reaches the change at runtime, so keep the repository's full required test
gate.

#### Status fields

| Field | Meaning |
|---|---|
| `db_path` | Exact UTF-8 index path. A non-UTF-8 path gives `db_path: null` and `db_path_error: non_utf8_path` |
| `extractor_contract_current`, `concept_contract_current` | Separate flags. The concept flag is false after a failed corpus update or a normalization change, even when the structural graph is current |
| `history_freshness` | `fresh`, `stale`, `incomplete`, `snapshot_changed` or `unknown`. `history_freshness_error` names a failed or work-limited scan |
| `freshness_basis: path_and_mtime` | Added, deleted, older and newer mtimes are stale. A content change that keeps the exact mtime needs `mastermind index --force` |
| `stale_files`, `stale_files_truncated` | Counts up to 100 stale paths and marks a larger set. A nonzero value means the next structural query refreshes a managed index, or a custom index needs `mastermind index` |
| `freshness_error` | The scan could not count. `stale_files` is then 1, so older clients fail closed |
| `unindexable_sources` | Up to 20 listed sources that are not regular files, such as symlinks. They never index, and count in `stale_files` only while the index still holds rows for them |
| `skipped_submodules` | Submodules the inventory did not index |

### Compiler and external evidence

| Tool | Args | Returns |
|---|---|---|
| `mmcg_semantic` | `symbol`, optional `top` (default 100, max 500) | SCIP definitions, references, implementations and type definitions with provenance. Without an overlay it returns `fallback_active: true`. Each collection reports an exact stored `total` when known, returned rows, truncation and `omitted_stale`. See [SCIP overlay](mmcg.md#optional-scip-semantic-overlay) |
| `mmcg_facts` | Optional `path` (default `.`), `top` (default 100, max 400) | Normalized `mastermind-facts/v1` annotations and relationships with the capability contract, repository and revision identity, provenance, source state and limits. See [fact-ingestion SDK](../fact-ingestion-sdk.md#read-and-replace-datasets) |
| `mmcg_team_map` | `manifest` | A bounded `mastermind-team/v1` graph over pinned local indexes. The manifest must be authorized by `MMCG_TEAM_MANIFEST` and `MMCG_TEAM_MANIFEST_SHA256`. See [team graph](../team-graph.md#use-through-mcp) |

### History, documentation and context

FTS queries accept plain hyphenated terms such as `cold-load`, which are quoted
as phrases. Explicit phrases, Boolean operators, column filters, NEAR and prefix
markers keep their FTS meaning. This applies to task, history and document
search and to project-context queries. Every match is retrieval evidence, zero
matches do not mean there is no relevant record, and the Markdown file stays
the source of truth.

| Tool | Args | Returns |
|---|---|---|
| `mmcg_tasks` | `query`, optional `top` (default 10, max 50) | Ranked canonical task specs (`.mastermind/tasks/<NNN>-<name>/spec.md`) with titles, excerpts, coverage, skipped artifacts and live `freshness`. Top-level `_` files and bare `.md` files under `tasks/` are excluded |
| `mmcg_history` | `query`, optional `kind`, `top` (default 10, max 50), `document_graph` | Ranked durable history. See [history search](#history-search) |
| `mmcg_docs` | `query`, optional `top` (default 10, max 50) | Markdown sections from the history corpus, root `README.md` and `docs/**/*.md`, with heading chains, line ranges, excerpts, counts and live freshness. Headings inside comments, raw HTML, code and blockquotes do not split sections. An index from an older section parser reports `section_extractor_current: false` and withholds excerpts until `mastermind index .`. New README and docs inputs with detected secret patterns are skipped |
| `mmcg_project_profile` | Optional `query` (FTS5, max 256 characters), `top` (default 12, max 32) | Section excerpts and claim candidates from the indexed root `CONTEXT.md`. See [project profile](#project-profile) |
| `mmcg_context` | `since`, `role`, optional `paths`, `workflow`, `query`, `budget_tokens` (default 8000, 1024-16000) | A read-only preview of code, project, documentation, granted personal advice and past tasks. No refresh, mining, model call or delivery receipt. See [context packets](persona-context.md) |
| `mmcg_profile` | Optional `paths` (max 64), `role` (`planner`, `executor`, `auditor`), `workflow`, `budget_tokens` (256-8000, default from `mastermind init --profile-budget`, else 4000) | The user's reviewed global profile for this client and project. See [profile access](persona.md#profile-access) and [profile delivery](persona.md#profile-delivery) |
| `mmcg_mining_submit` | `ticket_id` from the current prompt hook, `candidates` (0-2 objects with `when`, `behavior`, `exception`, `evidence_kind`, `quote`) | Stages current-task candidates that cite exact user prose, or an empty list. See [mining in the current task](persona.md#mining-in-the-current-task) |

#### History search

`mmcg_history` searches `CONTEXT.md`, `CONTEXT-archive-*.md`, canonical task
specs, executor reports, audits, `.mastermind/releases/*.md`, task-local release
notes, lessons and Markdown decisions under conventional ADR directories. The
CLI forms are `mastermind history <query>` and `mastermind query history`.

| Field or input | Meaning |
|---|---|
| `kind: architecture_decision` | Exact filter for ADRs |
| `candidate` lessons | Unresolved signals, not active guidance |
| `indexed_total` | Exact for the admitted FTS corpus only. Skipped or truncated Markdown stays outside it |
| `skipped_artifacts`, `corpus_truncated` | What the corpus left out |
| `freshness` | `fresh`, `stale`, `incomplete`, `snapshot_changed` or `unknown`. An unavailable, interrupted or foreign scan is `unknown`, a work-cap omission is `incomplete` |
| Limits | 1 MiB per artifact, 5,000 artifacts, 32 MiB of admitted text |
| `document_graph` | A packet under `.mastermind/research`. The response adds a separate live no-follow `document_graph` check. It writes nothing to SQLite, never changes `verification: unverified`, and its status is separate from index freshness. The CLI form is `mastermind history <query> --document-graph <path>` |

The inventory binds path, kind, length, content digest, skipped state and
truncation state for every artifact.

#### Project profile

`mmcg_project_profile` reads only the indexed root `CONTEXT.md`.

| Output | Contract |
|---|---|
| Section excerpts | Headings with full-section line ranges. A snippet may cover part of its section. `excerpt_truncated` and `heading_truncated` mark shortened text |
| `claim_candidates` | Only one-line `Decision` and `Status` fields in decision-log entries, with decision-line citations, record and file digests, extractor version and `status: candidate` |
| `source_status` | Repeats the Markdown value. `review_status` stays `unknown`, because nobody reviewed it here |
| Stale or incomplete history, section or claim extraction | All text is withheld. An older index needs `mastermind index .` |
| Secret-like or malformed decision entries | Extraction becomes incomplete. The secret check is a heuristic |

Code links in `CONTEXT.md` are not checked. The person's global profile stays
separate in `mmcg_profile`.

## Bounded role briefs

`mmcg_brief` and `mastermind brief` use the same builder. CLI JSON is the MCP
`structuredContent` packet, and text is a rendering of the same fields.

| Field | Content |
|---|---|
| `schema_version`, `repository_content_untrusted`, `role`, `freshness`, `baseline`, `scope`, `budget` | Packet identity and checks. Structural and history freshness have separate tokens and statuses |
| `changes` | Changed files and symbols |
| `callers` | Each with up to eight changed-symbol seeds, exact seed count and truncation, name-collision count and edge precision |
| `api_crossings` | Changed seed, changed and impacted components, impacted symbol and minimum static depth. Structural candidates only |
| `tests` | Each with up to eight direct, transitive or same-component evidence records, with count and truncation |
| `disciplines` | Count of change-impact paths classified before budget admission, rule names and bases, changed-file counts, the unclassified count and any scope boundary. No path samples |
| `history` | At most one quoted OR query. Path, kind, query-local rank and the matched lexemes. Stemming can make a lexeme differ from the query term |
| `citations`, `omitted`, `limits`, `precision_notes` | `omitted` separates source limits, rejected unsafe content and budget admission per collection. A null `total` with `source_limit_exact: false` is a lower bound |

Repository paths and symbol names are capped and escaped as untrusted data.
Source bodies, signatures, literals, defaults, history titles and excerpts are
left out.

| Cap | Value |
|---|---:|
| Changed files / changed symbols | 100 / 100 |
| Callers / seeds per caller | 100 / 8 |
| API crossings | 50 |
| Tests / evidence records per test | 50 / 8 |
| History citations / derived history terms | 10 / 8 |

| Role | Admission order |
|---|---|
| `planner` | changes, API crossings, callers, history, tests |
| `executor` | changes, API crossings, tests, callers, history |
| `auditor` | tests, API crossings, callers, changes, history |

The budget estimate is `ceil(serialized MCP tool-result bytes / 4)`. It counts
the escaped JSON text and the structured copy, and leaves out only the outer
JSON-RPC frame and request ID. Discipline labels stay in every packet, and an
incomplete change-file scope sets `scope_incomplete_reason` and keeps the brief
partial.

| Failure | Meaning |
|---|---|
| `{"code":"budget_too_small","minimum_tokens":N}` | Even the empty packet does not fit the budget |
| `invalid_arguments`, `invalid_ref`, `root_mismatch` | Bad input |
| `index_stale`, `schema_incompatible`, `snapshot_changed`, `index_busy` | Index state. Managed drift gets one refresh and one retry under the original deadline. Custom indexes stay read-only |
| `work_limit_exceeded`, `cancelled` | Budget or cancel |
