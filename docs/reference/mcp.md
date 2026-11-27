# MCP server

`mastermind serve` runs the Mastermind MCP server over stdio. It exposes 37 tools
over the local index: 22 queries that may refresh the managed index first, 14
read-only operations and `mmcg_mining_submit`, which stages source-cited
proposals. There is no arbitrary SQL or executable plugin. Tool annotations are
advisory and tool content is untrusted repository data, so neither grants a
permission. [Integrations](../integrations/generic-mcp.md) shows how to connect
a client, and the [CLI and index reference](mmcg.md) covers indexing.

## Protocol contract

A request whose `_meta` names a protocol version, and every `server/discover`
call, uses the per-request lifecycle. So does any request with `_meta` before
`initialize`. Other requests use `initialize`.

| Lifecycle | Revisions | Version choice |
|---|---|---|
| `initialize`, then `tools/list` and `tools/call` | `2025-11-25`, older `2024-11-05` | `2024-11-05` when requested, `2025-11-25` for anything else |
| Per-request `_meta`, no `initialize` | `2026-07-28` | Each call carries `io.modelcontextprotocol/protocolVersion` and `io.modelcontextprotocol/clientCapabilities` in `_meta`. Another version gets `-32022` with `supported` and `requested`. Missing or malformed metadata gets `-32602` |

`server/discover` lists only `2026-07-28`. Per-request results add
`resultType: "complete"` and the server info in `_meta`.

| Rule | Contract |
|---|---|
| Result shape | `2025-11-25` and `2026-07-28` results carry compact JSON text and an object `structuredContent`. `2024-11-05` results are content-only |
| Errors | Tool and input errors use `isError: true`. Malformed requests and internal failures use JSON-RPC errors |
| Input frame | At most 1 MiB. A larger frame closes the connection |
| Response | At most 8 MiB for the whole JSON-RPC response. A larger result asks the caller to narrow the query |
| Budget and cancel | See [work budgets](mmcg.md#work-budgets-timeouts-and-cancellation) |

## Managed index refresh

Structural tools, concept queries, review preparation and completion, and
briefs share one freshness check with the CLI. Each call makes at most one
refresh attempt under its original deadline, after it checks the repository root
against the stored index identity.

| Condition | Result |
|---|---|
| Source files or extractor contract drifted | One refresh of `.mastermind/mmcg.db`, then the query runs |
| More than 100,000 source files or 512 MiB of source | `refresh_limit_exceeded`, with no partial refresh |
| Rebuild does not fit the request budget | `index_rebuild_required`. Run `mastermind index .` or raise `MMCG_QUERY_BUDGET_MS` |
| Another writer holds the index | `index_busy` with `retryable: true`. Retry the same call |
| Failed or unavailable refresh | `index_stale` with coverage fields and `stale_reason` (`structural`, `concept` or `history`). History-only drift also sets `history_freshness: stale` |
| Listed source that is not a regular file | Never indexes and does not make the index stale. Up to 20 such paths are listed in `unindexable_sources` |
| Missing, non-regular or non-UTF-8 tracked path | Makes the inventory incomplete, so the index is stale |
| `mmcg_change_impact` and `mmcg_test_impact` after this check | Also compare every source that differs from the commit the index recorded, as [CLI `impact`](mmcg.md#change-impact-freshness) does. A failure is `{"error": "index_stale"}`, or `{"error": "index_missing"}` for an index that never indexed a repository |

A custom `--index` opens read-only, and the server never creates, migrates or
writes it. Refresh it with `mastermind --index PATH index ROOT`. `serve` reads a
private snapshot taken at start, so after `index` rewrites that file the tools
return `index_replaced` until you restart the server. An incompatible custom
schema returns `schema_incompatible`.

Source keys are repository-relative with `/` between components. A non-UTF-8
component, or a backslash in a Unix file name, is rejected so it cannot take
another file's key.

## MCP tools

Collection responses report exact `total`, returned `count`, `truncated`,
`row_limit`, the effective filters and `precision_notes`, also when empty.
Where a row says the CLI keeps a complete listing, the matching
`mastermind query` command has no MCP row cap.

### Read and reuse source ranges

`mmcg_read` takes a repository-relative `file`, a task or session label in
`task`, and optional inclusive `start_line` and `end_line`. It reads current
bytes whatever the index freshness, and returns the file SHA-256, absolute-line
`segments`, an opaque `receipt` and continuation fields.

| Limit | Value |
|---|---|
| Default range / new lines per reply | 80 / 200 |
| File | UTF-8 regular file of at most 1 MiB. Hidden path components and symlinks are rejected |
| Ranges per receipt | 128. Past that, `receipt_coverage_truncated` is set |
| Receipts held | 128 in memory, never stored or shared across server processes |

| Request | Delivery |
|---|---|
| No `previous_receipt` | Requested text, with pagination |
| Matching receipt for the same task and file | Only missing ranges. `reused_ranges` names the omitted text |
| Changed file, other task or file, expired receipt or restarted server | Requested text again, with `reuse_status` saying why |

Pass a receipt only while its text is still in the task's context, and drop it
after a handoff or compaction. When `range_truncated` is true, continue at
`next_line`. For an external index, bind the root explicitly:

```sh
mastermind --index /absolute/index/mmcg.db serve --root /absolute/repository
```

### Effort selection

| Tool | Args | Returns |
|---|---|---|
| `mmcg_effort_route` | Optional `agent`, `task_family` (each at most 80 UTF-8 bytes), `read_only`, `bounded_scope`, `stable_inputs`, `lexical_repo_map`, `risk` (`low`, `high`, `unknown`), `user_effort` (`low`, `medium`, `high`, `xhigh`, `max`) | Requested effort, reason, Codex and Claude argument arrays, and `codex_spawn_args` with `fork_turns: "none"`. Missing context keeps high effort and an explicit human choice wins. `applied` and `quality_qualified` stay false |

The tool reads no source and writes no configuration. The CLI form is
`mastermind effort-route --request route.json`, up to 16 KiB. See
[effort routing](../guides/effort-routing.md).

### Symbol lookup

| Tool | Args | Returns |
|---|---|---|
| `mmcg_search` | `name` or `names` (1-8), optional `kind`, `language`, `collapse_partials` (default `true`), `top` (default 100, max 200, or 10 and 25 per name in a batch) | Exact-name candidates. A batch reads one snapshot and fails with `snapshot_changed` if the index changes. With collapsing, at most 500 raw declarations per query are grouped. At that cap `total` is null with `truncation_reason: raw_work_limit` |
| `mmcg_callers` | `name`, optional `language`, `edge_kind` (default `calls`), `top` (default 100, max 500) | Containing functions that reference `name`, each counted once. `imports` gives importers, `references` gives function-value and Rust macro-body usages |
| `mmcg_callees` | `name`, optional `language`, `edge_kind`, `file`, `line` (needs `file`), `top` (default 100, max 500) | Outgoing names from one definition. `match_status` is `matched`, `ambiguous` (bounded `candidates`, no edges) or `not_found` |
| `mmcg_impact` | `name`, optional `max_depth` (1-10, default 2), `language` | Transitive candidates through `calls` and `references`, at most 5,000 rows |
| `mmcg_imports` | `file`, optional `top` (default 200, max 500) | Static imports with `name`, full `path` and `line` |
| `mmcg_imported_by` | `query`, optional `match` (`name` or `path`), `language`, `top` (default 200, max 500) | Files whose static imports reference the name or path |
| `mmcg_symbols_in_file` | `file`, optional `top` (default 200, max 500) | Symbols in source order |
| `mmcg_outline` | `file`, optional `top` (default 200, max 500) | Parent-preserving symbol tree, at most 64 levels. `depth_limit` or `hierarchy_incomplete` marks a partial tree |
| `mmcg_files` | Optional `prefix`, `language`, `top` (default 200, max 500) | Indexed files with symbol counts |
| `mmcg_recent_changes` | `since` (`2h`, `30m`, `1d`), optional `top` (default 200, max 500) | Files whose stored mtime is between `window_start_unix_ms` and `as_of_unix_ms` |
| `mmcg_change_class` | `file` | `structural`, `cosmetic` or `first-seen` for the last change, from an FNV-1a hash of the parsed shape |
| `mmcg_unreferenced` | Optional `kind`, `language`, `top` (default 100, max 500) | Symbols with no indexed reference candidates. See [limitations](mmcg.md#heuristic-query-semantics) |
| `mmcg_api_surface` | `prefix`, optional `language`, `top` (default 100, max 500) | Symbols under `prefix` with a reference candidate from outside it |
| `mmcg_concept` | `query`, optional `top` (default 10, max 50), `include_tests` | Lexical symbol candidates. See [concept search](concept-search.md#search-by-concept) |
| `mmcg_concept_prepare` | `query`, optional `top`, `include_tests`, `hints`, `request_contract` | A source-bound review packet. See [operation review](concept-search.md#review-operation-relevance) |
| `mmcg_concept_finish` | Unchanged `packet`, complete `assessment` | Rechecks source and orders the useful page. The calling agent owns the judgment |

### Architecture and change analysis

| Tool | Args | Returns |
|---|---|---|
| `mmcg_map` | Optional `path` (default `.`), `depth` (1-6, default 2), `top` (1-100, default 20), `production_only` | Schema-v1 briefing: entry points, components, boundaries, hotspots and import cycles. See [map scope and caps](#map-scope-and-caps) |
| `mmcg_temporal` | `since`, optional `root`, `path`, `depth` (1-5), `top` (1-100), `production_only`, `codeowners` | Delta between a Git baseline and the index. See [temporal graph](mmcg.md#temporal-graph) |
| `mmcg_centrality` | Optional `prefix`, `language`, `kind`, `top` (default 20, max 200) | Symbols ranked by distinct callers. Same-name collisions can pool |
| `mmcg_dependency_cycles` | Optional `language`, `min_size` (default 2), `top` (default 50, max 200) | Import cycles. See [cycle limits](#dependency-cycle-limits) |
| `mmcg_symbols_changed_since` | `git_ref`, optional `root`, `top` (default 100, max 500) | Symbol diff against a Git ref: `files_in_diff`, `added`, `removed`, `signature_changed`, `errors`, each capped at `top` with exact totals in `coverage`. At 10,000 files totals are null and `source_truncated` is set |
| `mmcg_change_impact` | `since`, optional `root`, `depth` (1-5), `top` (1-500) | Staged, unstaged and untracked changes: changed symbols, dependency candidates, component crossings, ranked test candidates and `disciplines` |
| `mmcg_test_impact` | `since`, optional `root`, `depth` (1-5), `top` (1-500) | The test part of `mmcg_change_impact`. See [test candidates](#test-candidates) |
| `mmcg_brief` | `role` (`planner`, `executor`, `auditor`), `since`, optional `root`, `budget_tokens` (256-8,000, default 2,000) | One role packet. See [bounded role briefs](#bounded-role-briefs) |
| `mmcg_status` | none | Index path, counts, contract flags and freshness. See [status fields](#status-fields) |

#### Map scope and caps

`path` is a literal prefix, so `%` and `_` are plain bytes. `production_only`
leaves out test, fixture, example, generated and vendor path segments and test
file names (`test_*`, `*_test.*`, `*_tests.*`, `*.test.*`, `*.tests.*`,
`*.spec.*`, `*Test.*`, `*Tests.*`). The map does not check source freshness.

| Cap | Value |
|---|---|
| Aggregation paths / scoped cycle edges | 50,000 / 50,000 |
| Languages / components / boundaries | 20 / 20 / 20 per component and 400 in total |
| Entry points / hotspots / cycles / cycle memberships | 50 / 100 / 50 / 500 |

| Truncation cause | Meaning |
|---|---|
| `path_work_limit` | Path-derived aggregates are partial |
| `language_limit`, `entry_point_limit`, `top_limit` | That section reached its cap |
| `top_probe` | A probe found one more row than the cap |
| `global_probe_limit` | The 401st boundary row left a component uncertain |
| `cycle_limit`, `cycle_membership_limit`, `cycle_and_membership_limit` | Cycle output was bounded |
| Cycle `work_limit` | Cycle analysis was skipped |

#### Dependency-cycle limits

| Bound | Result |
|---|---|
| More than 50,000 import file-pair edges | Analysis is skipped. `cycles` is empty, totals are null, `truncation_reason` is `graph_work_limit` |
| Output | Capped by `top` and 500 file memberships. A cycle that does not fit is omitted whole with `member_limit`, and totals stay exact |

Imports resolve by leaf name within one language family, and JavaScript,
TypeScript, TSX and Vue are one family. A same-named file can add an edge, so
check a cycle before you refactor, or use `mastermind query dependency-cycles`
for the complete list.

#### Test candidates

| Class | Evidence |
|---|---|
| Direct static | Changed tests and depth-1 graph tests |
| Transitive | Deeper graph tests |
| Heuristic | Same-component tests without graph evidence: `same_component_test_filename` or `same_component_test_attribute` |

Test attributes such as Rust `test`, `tokio::test` and `async_std::test` mark a
test whatever the file name. The classes rank candidates. They do not show that
a test ran or reaches the change, so keep the repository's full test gate.

#### Status fields

| Field | Meaning |
|---|---|
| `db_path` | Exact UTF-8 index path, or `null` with `db_path_error: non_utf8_path` |
| `extractor_contract_current`, `concept_contract_current` | Separate flags. The concept flag can be false while the graph is current |
| `history_freshness` | `fresh`, `stale`, `incomplete`, `snapshot_changed` or `unknown`, with `history_freshness_error` for a failed scan |
| `freshness_basis: path_and_mtime` | A content change that keeps the exact mtime needs `mastermind index --force` |
| `stale_files`, `stale_files_truncated` | Up to 100 stale paths. Nonzero means the next query refreshes a managed index |
| `freshness_error` | The scan could not count. `stale_files` is then 1 |
| `unindexable_sources`, `skipped_submodules` | Sources that are not regular files, and submodules left out |

### Compiler and external evidence

| Tool | Args | Returns |
|---|---|---|
| `mmcg_semantic` | `symbol`, optional `top` (default 100, max 500) | SCIP definitions, references, implementations and type definitions with provenance, or `fallback_active: true` without an overlay. See [SCIP overlay](mmcg.md#optional-scip-semantic-overlay) |
| `mmcg_facts` | Optional `path` (default `.`), `top` (default 100, max 400) | `mastermind-facts/v1` annotations and relationships with provenance and limits. See [fact-ingestion SDK](../fact-ingestion-sdk.md#read-and-replace-datasets) |
| `mmcg_team_map` | `manifest` | A `mastermind-team/v1` graph over pinned local indexes, allowed by `MMCG_TEAM_MANIFEST` and `MMCG_TEAM_MANIFEST_SHA256`. See [team graph](../team-graph.md#use-through-mcp) |

### History, documentation and context

FTS queries quote plain hyphenated terms such as `cold-load` as phrases.
Explicit phrases, Boolean operators, column filters, NEAR and prefix markers
keep their FTS meaning. Zero matches do not mean there is no relevant record,
and the Markdown file stays the source of truth.

| Tool | Args | Returns |
|---|---|---|
| `mmcg_tasks` | `query`, optional `top` (default 10, max 50) | Ranked task specs (`.mastermind/tasks/<NNN>-<name>/spec.md`) with excerpts and live `freshness` |
| `mmcg_history` | `query`, optional `kind`, `top` (default 10, max 50), `document_graph` | Ranked project history. See [history search](#history-search) |
| `mmcg_docs` | `query`, optional `top` (default 10, max 50) | Markdown sections from the history corpus, root `README.md` and `docs/**/*.md`, with heading chains and line ranges. An older section parser reports `section_extractor_current: false` and withholds excerpts. Docs with detected secrets are skipped |
| `mmcg_project_profile` | Optional `query` (max 256 characters), `top` (default 12, max 32) | Sections and claim candidates from the root `CONTEXT.md`. See [project profile](#project-profile) |
| `mmcg_context` | `since`, `role`, optional `paths`, `workflow`, `query`, `budget_tokens` (1,024-16,000, default 8,000) | A read-only context packet. See [context packets](persona.md#context-packets) |
| `mmcg_profile` | Optional `paths` (max 64), `role`, `workflow`, `budget_tokens` (256-8,000, default from `init --profile-budget`, else 4,000) | The reviewed personal profile. See [profile access](persona.md#profile-access) |
| `mmcg_mining_submit` | `ticket_id` from the prompt hook, `candidates` (0-2 objects with `when`, `behavior`, `exception`, `evidence_kind`, `quote`) | Stages candidates that cite exact user prose. See [mining in the current task](persona.md#mining-in-the-current-task) |

#### History search

`mmcg_history`, `mastermind history <query>` and `mastermind query history`
search `CONTEXT.md`, `CONTEXT-archive-*.md`, task specs, executor reports,
audits, `.mastermind/releases/*.md`, task release notes, lessons and Markdown
under `docs/adr`, `docs/adrs`, `docs/decisions`, `adr`, `adrs` and
`.mastermind/decisions`.

| Field or input | Meaning |
|---|---|
| `kind: architecture_decision` | Exact filter for ADRs |
| `candidate` lessons | Unresolved signals, not active guidance |
| `indexed_total` | Exact for the admitted corpus. `skipped_artifacts` and `corpus_truncated` say what was left out |
| `freshness` | `fresh`, `stale`, `incomplete` (work cap), `snapshot_changed` or `unknown` |
| Limits | 1 MiB per artifact, 5,000 artifacts, 32 MiB in total |
| `document_graph` | A packet under `.mastermind/research`, checked live and reported apart from index freshness. It never changes `verification: unverified`. CLI form `--document-graph <path>` |

#### Project profile

| Output | Contract |
|---|---|
| Section excerpts | Headings with full-section line ranges. `excerpt_truncated` and `heading_truncated` mark shortened text |
| `claim_candidates` | One-line `Decision` and `Status` fields of decision-log entries, with citations, digests and `status: candidate` |
| `source_status`, `review_status` | The Markdown value, and `unknown` because nobody reviewed it here |
| Stale or incomplete extraction | All text is withheld. An older index needs `mastermind index .` |
| Secret-like or malformed decision entries | Extraction becomes incomplete |

## Bounded role briefs

`mmcg_brief` and `mastermind brief` share one builder. CLI JSON is the MCP
`structuredContent` packet. Paths and names are escaped as untrusted data, and
source bodies, signatures, literals and history excerpts are left out.

| Field | Content |
|---|---|
| `schema_version`, `repository_content_untrusted`, `role`, `freshness`, `baseline`, `scope`, `budget` | Packet identity and checks, with separate structural and history freshness |
| `changes` | Changed files and symbols |
| `callers` | Up to eight changed-symbol seeds each, with seed count, collision count and edge precision |
| `api_crossings` | Changed seed, components, impacted symbol and minimum static depth |
| `tests` | Up to eight direct, transitive or same-component evidence records each |
| `disciplines` | Path classification counts and rule names, with no path samples |
| `history` | At most one quoted OR query with path, kind, rank and matched lexemes |
| `citations`, `omitted`, `limits`, `precision_notes` | `omitted` separates source limits, unsafe content and budget cuts. A null `total` with `source_limit_exact: false` is a lower bound |

| Cap | Value |
|---|---:|
| Changed files / changed symbols / API crossings | 100 / 100 / 50 |
| Callers / seeds per caller | 100 / 8 |
| Tests / evidence per test | 50 / 8 |
| History citations / history terms | 10 / 8 |

| Role | Admission order |
|---|---|
| `planner` | changes, API crossings, callers, history, tests |
| `executor` | changes, API crossings, tests, callers, history |
| `auditor` | tests, API crossings, callers, changes, history |

The budget estimate is `ceil(serialized MCP tool-result bytes / 4)`, counting
the JSON text and the structured copy. An incomplete change-file scope sets
`scope_incomplete_reason`.

| Failure | Meaning |
|---|---|
| `{"code":"budget_too_small","minimum_tokens":N}` | Even the empty packet does not fit |
| `invalid_arguments`, `invalid_ref`, `root_mismatch` | Bad input |
| `index_stale`, `schema_incompatible`, `snapshot_changed`, `index_busy` | Index state |
| `work_limit_exceeded`, `cancelled` | Budget or cancel |
