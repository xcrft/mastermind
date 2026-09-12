# mmcg — Mastermind Codegraph

Need the exact command, limit, schema, precision caveat, or MCP shape? This is
the source of truth for the `mmcg` engine. Start with
[Getting started](../getting-started.md) for installation or the
[project README](../../README.md) for the product overview.

`mmcg` is a Rust binary that builds a local structural index for Python,
TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust, C#, Go, Java, PHP, and C/C++.
It exposes the same indexed state through CLI, Lens, and MCP. The MCP surface
contains 30 tools: 21 non-destructive queries that may refresh the managed
derived index, 8 read-only queries, and one additive local scratchpad write.
The binary also provides spec gates, client setup, evidence ingestion, review
export, and style mining.

> npm installs the command as both `mastermind` and `mmcg`. Cargo installs
> `mmcg`. Examples below use `mmcg`; the command surface is identical.

## Find the exact contract

| You need to verify… | Section |
|---|---|
| Supported syntax and known extraction gaps | [What it indexes](#what-it-indexes) |
| Installation and commands | [Build from source](#build-from-source), [CLI usage](#cli-usage) |
| Compiler-resolved enrichment | [SCIP semantic overlay](#optional-scip-semantic-overlay) |
| External revision-bound evidence | [Fact-ingestion SDK](#declarative-fact-ingestion-sdk) |
| Multi-repository local view | [Local team graph](#local-team-graph) |
| Base-versus-head architecture | [Temporal graph](#temporal-graph-mmcg-temporal) |
| Local review UI and portable evidence | [Lens](#mastermind-lens-mmcg-ui), [review package](#pr-evidence-package-mmcg-review-export) |
| Architecture rules | [Policy as code](#architecture-policy-as-code-mmcg-policy-check) |
| MCP protocol and tool schemas | [MCP server](#mcp-server-usage), [MCP tools](#mcp-tools) |
| Bounds and precision caveats | [Work budgets](#work-budgets-timeouts-and-cancellation), [limitations](#limitations) |
| Local natural-language symbol retrieval | [Local symbol concept search](#local-symbol-concept-search) |

## What it indexes

For each supported file, mmcg captures:

| Construct | Python | TS/JS | Rust | C# | Go | Java | PHP | C/C++ |
|---|---|---|---|---|---|---|---|---|
| Functions | ✓ `def` (`.py`/`.pyi`) | ✓ `function` | ✓ `fn` | ✓ method/local-fn | ✓ `func` | ✓ `method_declaration` | ✓ `function` | ✓ definitions + header declarations |
| Module constants | ✓ `FOO = ...` (direct module children only) | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| Methods | ✓ inside classes | ✓ inside classes | ✓ inside `impl`/`trait` | ✓ inside classes/etc | ✓ `func (r T) M()` | ✓ inside classes | ✓ inside classes | ⚠️ inside classes + `T::m()` def |
| Types | ✓ `class` | ✓ `class`, `interface` | ✓ `struct`, `enum`, `trait` | ✓ `class`, `struct`, `interface`, `record`, `enum` | ✓ `struct`, `interface` | ✓ `class`, `interface`, `enum`, `record` | ✓ `class`, `interface`, `trait`, `enum` | ✓ `class`, `struct`, `union`, `enum` |
| Calls | ✓ | ✓ + `<Component />` JSX | ✓ + `Mod::foo()` | ✓ + `Type.Method()` | ✓ + `pkg.Func()` + `Foo{}` | ✓ + `new Foo()` | ✓ + `Foo::bar()` + `new Foo` | ⚠️ + `new Foo` (no template inst) |
| Decorators/attributes | ✓ `@pytest.fixture` | n/a | ✓ `#[test]` | ✓ `[Fact]` | n/a | ✓ `@Test`, `@GetMapping` | ✓ `#[Test]` (PHP 8) | n/a |
| Imports | ✓ `import`/`from` | ✓ ES module forms | ✓ `use` paths | ✓ `using` | ✓ `import` | ✓ `import` | ✓ `use` | ⚠️ `#include` path resolved to indexed headers for file graphs |
| Macros | n/a | n/a | ✓ `println!` | n/a | n/a | n/a | n/a | ❌ invisible — see Limitations |
| **FQ path** | `collections.abc.Iterable` | `'pkg'::default` | `foo::bar::Baz` | `System.Collections::*` | `net/http::*` | `java.util.List` | `App\Foo` | `vector::*` |

Each file gets a synthetic `<module>` symbol (kind `module`) that owns module-scope imports and top-level statements.

### Language coverage

Honest per-language summary — what the indexer captures and where it stops:

| language | symbols | calls | imports | known gaps |
|---|---|---|---|---|
| Python | function, method, class, constant (`.py` and `.pyi`) | ✓ direct + `obj.method()` (capital-letter receiver heuristic) | ✓ `import`/`from … import` with aliases and `as` rebinds | star-import expansion not tracked; dynamic `getattr` dispatch invisible; `__all__`-filtered re-exports not linked |
| TypeScript | function, arrow-fn, method, class, interface, type-alias | ✓ + `new Foo()` constructors + method calls + `<Component />` JSX usage | ✓ ES named / default / namespace + re-exports | anonymous default exports lose name; `export * from` re-exports not expanded to member level; JSX component detection is the uppercase-tag convention |
| JavaScript | function, arrow-fn, method, class | ✓ same walker as TS (TS-only node kinds skip silently), including JSX in `.jsx`/`.js` | ✓ ES named / default / namespace | no CommonJS `require()` as import edge; same gaps as TS minus interface / type-alias |
| Vue SFC | component (the file), plus every script symbol via the TS/JS walker | ✓ `<BaseButton />` and `<base-button />` template usage, normalized to PascalCase | ✓ the script block's ES imports | template expressions (`@click="bump"`, `:prop="expr"`) are attribute text, not parsed; auto-imported components (`unplugin-vue-components`) have no import edge; `<script>` and `<script setup>` merge into one symbol set |
| Rust | function, method, struct, enum, trait, impl-block, mod, macro-call | ✓ + `Crate::fn()` scoped calls + `macro!` invocations | ✓ `use` paths with aliases and globs | proc-macros invisible at parse time; `derive` traits stored as decorator not call edge; glob `use foo::*` recorded as `*` (no member expansion) |
| C# | class, struct, record, interface, enum, method, property, namespace | ✓ + `new Foo()` constructors + method calls | ✓ `using` directives + type aliases | anonymous lambdas unnamed; LINQ extension calls not tracked individually; `partial class` stored per-file (collapsed on query by default) |
| Go | function, method, struct, interface, type | ✓ + composite literals `Foo{}` + pkg-qualified `pkg.Fn()` | ✓ `import` paths with aliases, blank identifier, dot imports | goroutine launches not marked semantically; anonymous closures unnamed; build tags stored as decorator |
| Java | class, interface, enum, record, method, constructor | ✓ + `new Foo()` + method-call expressions | ✓ `import` declarations including static + wildcards | anonymous inner classes not tracked; lambda bodies unnamed; annotation processors invisible |
| PHP | namespace, class, interface, trait, enum, method, function | ✓ + `new Foo`, `Foo::bar()`, `$this->method()` | ✓ `use` with aliases + grouped `use App\{A, B as C}` form | magic methods (`__get`, `__call`) tracked as symbols but call targets unresolved; `call_user_func` target invisible |
| C/C++ | function/method definitions and declarations, class, struct, union, enum | ⚠️ best-effort — no preprocessor, no semantic analysis | ⚠️ `#include` spelling is retained and resolved to indexed files for maps/cycles | macros invisible (`TEST(Suite, Name)` parsed as a call not a def); template instantiations not tracked; header/source split produces duplicate rows (no dedup); ADL/overload not resolved |

### Path format per language

- **Python:** dotted. `from collections.abc import Iterable as Iter` → name=`Iter`, path=`collections.abc.Iterable`
- **TS/JS:** module-source + leaf separated by `::`. `import { foo as bar } from './a'` → name=`bar`, path=`./a::foo`. Defaults: `<src>::default`. Namespace: `<src>::*`.
- **Rust:** Rust-style `::`. `use foo::bar::Baz as Q` → name=`Q`, path=`foo::bar::Baz`. Wildcards: `foo::*`.
- **C#:** namespace + `::*` wildcard. `using System.Collections.Generic;` → name=`Generic`, path=`System.Collections.Generic::*`. Aliases (`using X = Y.Z`) take the right side.
- **Go:** package path + `::*`. `import f "fmt"` → name=`f`, path=`fmt::*`. `_` / `.` aliases fall back to path leaf.
- **Java:** dotted. `import java.util.List` → name=`List`, path=`java.util.List`. Wildcards: `java.util::*`. `import static …` keeps the symbol leaf.
- **PHP:** backslash-namespaced. `use App\Foo as Bar` → name=`Bar`, path=`App\Foo`. Grouped (`use App\{A, B as C}`) expands per-item.
- **C/C++:** `#include` produces an import edge with the header filename and full path: `#include <vector>` → name=`vector`, path=`vector::*`; `#include "sub/dir/x.h"` → name=`x.h`, path=`sub/dir/x.h::*`. `using std::vector` → name=`vector`, path=`std::vector`. `using namespace ns` → name=`*`, path=`ns::*`.
- **Calls:** for `obj.foo()`, path is the literal `obj.foo` from source (no type resolution — see Limitations).

Source discovery honors repository, parent, global, and `.git/info/exclude`
Git ignore rules even outside a Git worktree. `.ignore` files are honored too.
Files already tracked by Git remain index candidates even if a broad ignore
rule now matches them; ignore rules still exclude untracked files. Supported
extensions are matched case-insensitively, Python type stubs (`.pyi`) are
parsed as Python, and UTF-16 LE/BE sources with a BOM are decoded before parsing.
These directories are always skipped: `.git`, `.mastermind`, `.venv`, `venv`,
`__pycache__`, `node_modules`, `target`, `dist`, `build`, `.tox`, `.pytest_cache`,
`.mypy_cache`, `.ruff_cache`, `.next`, `.turbo`, `.cache`.

## Design scope

The default index is intentionally smaller than a compiler database. It uses
Tree-sitter so a mixed-language repository can be indexed locally without
installing each language toolchain. SQLite gives CLI, MCP, and Lens a shared
bounded query surface instead of repeatedly rescanning source text.

- Ten language families share one source-discovery and storage contract.
- Optional SCIP facts add compiler-resolved evidence without replacing the
  syntactic graph.
- External producers submit validated facts; they cannot execute inside the
  process or write SQLite.
- MCP exposes 30 bounded tools: 21 non-destructive queries that may refresh the
  managed derived index, 8 read-only queries, and the additive, gitignored
  `mmcg_scratchpad_append` write.

## Performance model

- **Parsers**: tree-sitter (C, vendored — no system tree-sitter required)
- **Parallelism**: `rayon` parses files in parallel; writes serialize through a single SQLite connection (WAL mode)
- **Bounded batching**: at most 64 parsed files are retained before the single SQLite writer commits them; each file uses one transaction via `Store::commit_file`
- **Source admission**: repository-relative descriptors are opened without following symlinks or Windows reparse points, every parent component stays beneath the repository capability, and identity is rechecked after the read. Source-looking files above 5 MiB or containing a NUL byte in the first 8 KiB are skipped before parsing; UTF-16 BOMs are admitted and decoded
- **Storage**: SQLite with indexes on `symbols.name`, `edges.from_id`, and `edges.to_name`

Index time depends on repository size, filesystem, hardware, and whether the run
is incremental. Use `mmcg index . --force` for a cold parse measurement and
record the emitted file/symbol/edge counts with the timing; do not compare a
cold run with an incremental no-op. Maintainers can run `just benchmark-index`
for a reproducible synthetic cold/warm/incremental report including peak process
RSS. See [Indexing benchmarks](../benchmarks.md) for current measurements,
parameters, ranges, and limitations. The benchmark is a regression aid, not a
machine-independent CI threshold.

## Build from source

Requires Rust 1.96+ ([rustup](https://rustup.rs/)). No system libraries — SQLite and tree-sitter are bundled at compile time.

```bash
cd mcp/servers/mmcg
cargo install --path .   # installs mmcg into ~/.cargo/bin/
```

The recommended install for most users is `npm install -g @xcraftmind/mastermind` (prebuilt binary — no toolchain needed).

## CLI usage

```bash
# Build/refresh the index for the current directory (incremental — skips unchanged files)
mmcg index

# Or for a specific path
mmcg index ~/code/my-project

# Force full re-index — re-parses everything regardless of mtime
mmcg index --force

# Optionally add compiler-resolved SCIP evidence without replacing the graph.
mmcg enrich --scip index.scip
mmcg query semantic "scip-clang . my-package . PaymentService#charge()." --top 100
mmcg query facts --top 1
mmcg enrich --facts facts.json
mmcg query facts --path src --top 400

# Watch a directory and re-index on file changes (long-running, also incremental)
mmcg watch

# Show what's in the index
mmcg status

# Search durable decisions, reports, audits, and lessons. `why` renders an
# evidence envelope with history freshness, skipped counts, and truncation.
# It never invents rationale absent from the records.
mmcg history "webhook dedupe"
mmcg history "runtime boundary" --kind audit
mmcg why "why is webhook dedupe durable?"

# Build one bounded schema-v1 project map with text, JSON, Mermaid, or SARIF projections
mmcg map . --format text
mmcg map src --format json --depth 2 --top 20
mmcg map . --format mermaid
mmcg map . --format sarif > mastermind-map.sarif

# Analyze baseline vs staged, unstaged, and untracked changes.
mmcg impact --since main --format text --depth 3 --top 100
mmcg impact --since HEAD~1 --format json
mmcg impact --since main --format sarif > mastermind-impact.sarif

# Compare bounded architecture snapshots over time.
mmcg temporal --since main
mmcg temporal --since HEAD~5 --path services/payment --format json

# Evaluate repository-owned architecture rules. Violations and incomplete
# evidence both exit non-zero.
mmcg policy check --since main
mmcg policy check --since main --format sarif > mastermind-policy.sarif

# Serve the local, read-only diff-first Lens UI on an ephemeral loopback port.
mmcg ui --since main
mmcg ui --since origin/main --path src --depth 2 --top 50 --production-only
mmcg ui --since main --document-graph .mastermind/research/session-evidence-v2.json
mmcg ui --since main --sarif semgrep.sarif --sarif codeql.sarif \
  --coverage lcov.info --coverage cobertura.xml \
  --junit junit.xml --otel traces.json

# Health-check the project setup (index, gitignore, CLAUDE.md, MCP config,
# `mmcg serve` handshake, and installed Mastermind agent runtime contracts).
# Agent checks cover model, explicit tools, bounded turns/effort, MCP
# registration, exact known mmcg grants, and prompt-required tools.
# Exit code 1 if any check fails — wire into CI.
mmcg doctor                                          # human-readable report
mmcg doctor --json                                   # machine-parseable

# Audit owned workflow wiring without executing prompts, tools, or models.
mmcg workflow audit --root .
mmcg workflow audit --root ~/.claude --json

# Pre-execution gate — verify a spec before handing off to the executor.
# Catches missing symbols, missing files, empty mandatory sections, snapshot
# drift, blast-radius warnings. Exit 1 on errors.
mmcg verify-spec .mastermind/tasks/042-feature/spec.md
mmcg verify-spec .mastermind/tasks/042-feature/spec.md --strict         # contract mode: require frontmatter scoping + a verify cmd + index
mmcg verify-spec .mastermind/tasks/042-feature/spec.md --require-index  # fail (don't skip live checks) when no index

# Post-execution audit — compare spec contract against actual repo state.
# Diffs <git-ref> (typically `main` or merge-base) against the WORKING TREE, so
# uncommitted and untracked work counts — the audit runs before the commit step.
# Flags scope creep, pre-edit snapshot drift, vanished symbols. Exit 1 if
# verdict is `broken`. (`--bundle` seals a commit-range diff instead and still
# requires baseline != HEAD.)
mmcg audit-spec .mastermind/tasks/042-feature/spec.md --since main

# Create a compact verified task (default), or a strict high-risk task.
mmcg new-spec "Add account recovery"
mmcg new-spec "Rotate signing keys" --mode strict

# Two-phase, client-neutral task controller.
#   pre  → verify spec, report risk, capture HEAD, write <task>/state.json
#   handoff → any implementation client writes <task>/executor-report.md
#   post → require and parse that report, audit against the baseline, write
#          <task>/audit.md, create <task>/history-review.md, update state, and
#          write .mastermind/releases/<task>.md on Held.
mmcg run-task .mastermind/tasks/042-feature/spec.md             # hand-off semantics
mmcg run-task .mastermind/tasks/042-feature/spec.md --exec      # legacy Claude-only `claude -p` convenience
mmcg run-task .mastermind/tasks/042-feature/spec.md --reset     # repeat pre-flight; preserve original baseline and counter
mmcg run-task .mastermind/tasks/042-feature/spec.md --pre-only  # pre-flight only; same retry guarantees
mmcg run-task .mastermind/tasks/042-feature/spec.md --post-only # requires state
mmcg run-task .mastermind/tasks/042-feature/spec.md --allow-no-index  # docs-only / spec-only specs
mmcg run-task .mastermind/tasks/042-feature/spec.md --strict          # fold strict spec checks into pre-flight
mmcg run-task .mastermind/tasks/042-feature/spec.md --max-iterations 5 # raise the default budget (default 3)
mmcg run-task .mastermind/tasks/042-feature/spec.md --force-iteration  # bypass budget; deduplicated lesson candidate records the signal
# NOTE: without --allow-no-index, pre-flight hard-fails when the index is missing
# or empty. Gates without a codegraph degrade to file-existence + section checks
# only — mmcg's value comes from the structural truth layer, not the heuristics.
# Query errors or an incomplete dependency-cycle graph also block approval.
# --allow-no-index does not bypass failures in a populated index.

# Initialize a project. Stack detection informs drafting, while CONTEXT stays
# lean and stack-agnostic; commands and layouts belong in CLAUDE.md.
mmcg init
mmcg init --no-claude      # skip Claude-assisted context drafting
mmcg init --no-index       # scaffold without building the graph
mmcg init --no-global      # do not reconcile the npm Claude workflow bundle
mmcg init --no-seed-style  # do not enrich ~/.mastermind/style.md

# Build or refresh the user-global personal style profile. No init required.
mmcg miner profile .
mmcg miner profile . --author "Ada Lovelace"  # literal substring of author name/email
mmcg miner profile . --deep                    # explicit claude -p compatibility path
# --force intentionally replaces the whole profile, including preserved prose;
# it is not refresh.
# Existing profiles are capped at 1 MiB and must be regular, no-follow files.
# Publication is serialized and atomic; concurrent mines cannot lose a contribution.
# A manual edit is rebased with bounded retries and is never silently overwritten.
# Git history reads are bounded; an oversized patch sample shrinks by whole commits.
# --deep caps its prompt/output, rejects malformed sections, and times out after 180s.
# Subdirectories and linked worktrees share one repository contribution.
# Independent clones remain separate; repeated samples do not prove quality.

# Preview or apply one supported MCP client target.
mmcg setup claude --scope user                            # dry-run via native `claude mcp`
mmcg setup cursor --scope project --root . --write        # write .cursor/mcp.json
mmcg setup codex --scope user --write                     # user-only via native `codex mcp`
mmcg setup continue --scope project --root . --write      # owned mastermind.yaml
mmcg setup generic --scope project --config ./mcp.json    # explicit JSON target, dry-run

# Remove a setup. --scope project (default) deletes .mastermind/ + the project
# .mcp.json mmcg entry; --scope global de-registers via `claude mcp remove`;
# --scope all does both. Dry-run unless --force. Never touches CONTEXT.md/CLAUDE.md.
mmcg uninstall                                            # dry-run: project teardown plan
mmcg uninstall --force                                    # remove .mastermind/ + project MCP entry
mmcg uninstall --scope all --force                        # also de-register the global MCP entry

# One-shot queries (for agents, use the MCP server)
mmcg query search PendingFile
mmcg query callers commit_file
mmcg query callers SomeFn --edge-kind imports     # who imports the symbol
mmcg query callers callback --edge-kind references # function-value/macro references
mmcg query callees parse_one
mmcg query callees process --file src/second.rs --line 12  # select a returned candidate
mmcg query explain process --language rust          # diagnose definition and edge-count scope
mmcg query impact extract --depth 3
mmcg query files --prefix src/indexer
mmcg query outline src/store.rs                    # symbol tree of one file
mmcg query imports src/store.rs                    # indexed static imports of one file
mmcg query recent --since 2h                       # stored source mtimes in last 2 hours
mmcg query unreferenced --kind function            # dead-code candidates (review manually)
mmcg query api-surface src/runtime/                # symbols under prefix used externally

# Deterministic local concept retrieval (no embeddings or model calls)
mmcg concept "payment retry handler" --top 10
mmcg concept "payment retry handler" --top 10 --format json
```

### Explaining symbol queries

`mmcg query explain <name>` returns raw exact-name definitions and diagnostic
counts for `calls`. `--language` filters both definitions and incoming source
symbols. Returned language identifiers can be reused as filters, including
`tsx` for `.tsx` files and `typescript` for `.ts` files.

The response now has `schema_version: 2`. Compared with the previous unversioned
output, `callee_count` and `edge_precision` are nullable when there is no unique
definition. Consumers must check `match_status` before using that summary:

| `match_status` | `matched` | `callee_count` / `edge_precision` |
|---|---|---|
| `matched` | One raw definition | Its outgoing count and language precision; zero means no indexed call pairs were found |
| `ambiguous` | All same-name definitions after the language filter | Both `null`; use `query callees` with an exact candidate file and declaration start line |
| `not_found` | Empty | Both `null`; no outgoing measurement was made |

`caller_count_scope: "name_or_type_candidates"` identifies the unchanged incoming
count: distinct containing source symbols with compatible edges to the queried
name or type prefix. It is not specific to a definition and can be positive even
without an indexed definition. The language filter restricts incoming sources;
target-kind compatibility still considers all same-name definitions. Repeated
call sites in one source symbol count once. `callee_count` instead counts distinct
`(target name, call line)` pairs of the one matched definition. `edge_precision`
and `limitations` describe only that definition's outgoing extraction;
`precision_notes` retains general dependency and count-scope caveats in every
response. Empty results do not prove absence of runtime dependencies.

Partial-class declarations stay separate in `matched`; `query search` may group
compatible declarations. Storage and query failures exit with an error instead
of producing a successful JSON response with a zero count.

### Local symbol concept search

`mmcg_concept` and `mastermind concept` use one schema-v2 query builder over a
private derived corpus. Searchable fields are normalized symbol names,
repository-relative paths, bounded declaration shapes, and owned documentation
for an explicit first language set. Source bodies and raw comments/docstrings
are never persisted or returned. Quoted, character, raw, template, regex,
numeric, and default-value content is removed from declaration shapes before
persistence.

| Language | Documentation ownership |
|---|---|
| Rust | Consecutive outer `///` or `/** */` docs on the immediately owned item; attributes may sit between the docs and item. Inner `//!` docs belong only to their module. |
| Python | The first plain string-expression statement in a module, class, sync function, or async function body. |
| TypeScript / TSX / JavaScript | One immediately preceding `/** ... */` JSDoc block, including through an export wrapper. |

Blank-line gaps, ordinary/trailing comments, file/license headers, Python
f-strings, later or assigned strings, and JSDoc before another statement are
not attached. C, C++, C#, Go, Java, PHP, and Vue remain name/path/declaration-
shape only; precision notes report this supported matrix rather than implying
documentation coverage for every extractor.

Each admitted documentation candidate is stripped without rendering Markdown
or HTML and bounded to 2 KiB of raw UTF-8. The shared concept tokenizer then
persists at most 512 bytes of normalized search tokens per symbol and 64 KiB
per file, in stable symbol order. Private-key headers, AWS access-key shapes,
bearer authorization values, and non-placeholder api-key/secret/token/password
assignments omit the whole candidate before persistence. Index stats and meta
report supported languages, indexed-document count, credential omissions,
size truncations/omissions, and unsupported-language file count only. They do
not contain matched text.

The query is plain text up to 256 UTF-8 bytes. Shared normalization applies
Unicode lowercase without claiming canonical equivalence, splits punctuation,
paths, snake/kebab/camel/acronym transitions and letter/digit boundaries, and
admits at most 16 terms of 64 bytes each. Every term is escaped, quoted, and
joined by a fixed `AND`, so callers cannot supply FTS syntax, column selectors,
boolean operators, or prefix wildcards. `top` is an integer from 1 through 50.

SQLite FTS5 ranks with fixed BM25 weights: name 10, path 4, declaration shape
2, and documentation 1. The total tie order is score,
lowercase repository path, line, kind, name, then symbol ID. Scores are local
to one query, lower is better, and they are not confidence values. Each
candidate contains `name`, `kind`, `language`, `path`, `line`, a declaration
`signature_shape` capped at 256 bytes, `matched_fields`, `citation`, and
`score`. The envelope reports exact `indexed_total`, returned `count`,
`result_truncated`, observed `unsafe_candidates_omitted`, overall `truncated`
and `truncation_reason`, plus normalized `query_terms`, requested top,
freshness, limits, and precision notes. The count and ranked page are read from
one SQLite snapshot.

The corpus is an additive schema-v7 concept table, per-file count table, and
external-content FTS5 index. Documentation rows and their count-only metadata
are replaced or removed in the same file transaction as symbols and edges.
Symbol mutations dirty the independent normalization contract, including
writes from older schema-v7 binaries. Managed queries may perform one bounded
full refresh and retry when the extractor or concept contract drifts.
Finalization checks symbol/concept parity and FTS integrity before stamping the
extractor and normalization contracts current. Custom external indexes open
read-only, are never migrated, and return `index_stale` or
`schema_incompatible` when the required corpus is unavailable. Stable query
failures also include `invalid_arguments`, `snapshot_changed`,
`work_limit_exceeded`, `cancelled`, and `internal_error`.

This is deterministic local retrieval, not semantic inference: no embeddings,
model calls, network access, broad grep, or canonical-equivalence matching are
used. Returned repository strings are untrusted data. JSON preserves them as
JSON strings; human text renders syntax-forming characters as Unicode escapes.

### Workflow audit

`workflow audit` emits `schema_version: 1` with stable `nodes`, `edges`,
`diagnostics`, `limits`, `complete`, and `context_estimates`. Node IDs are
kind-prefixed (`agent:`, `skill:`, `model:`, `server:`, `tool:`, `artifact:`,
`writer:`). Edges identify their relation and precision. Human and JSON output
come from the same report. Exit 0 means complete input and no error diagnostic;
exit 1 means an error or incomplete input; clap usage errors remain exit 2.

The loader limits source/installed input to 128 agents, 512 skills, 256 KiB per
Markdown file, a 1 MiB manifest, 8 MiB aggregate text, 8,192 directory entries,
4,096 directories, 4,096 nodes, 16,384 edges, and depth 16. Per-component
limits cover 512 skill relations, 64 writes, 512 runtime grants, and 64 MCP
servers; the report also caps admitted writers at 512, diagnostics at 4,096,
and context estimates at 16,384. It enumerates directories through already
opened no-follow handles and rejects symlinks, non-regular files, path escapes,
identity changes during reads, non-UTF-8 Markdown, aliases, anchors, tags, merge
keys, duplicate keys, multiple YAML documents, and unknown workflow metadata.
Any skipped input sets `complete: false`; dependent negative findings are not
claimed from a partial inventory.

When an installed Claude role scopes `mmcg`, registration comes only from the
project `.mcp.json` beside `.claude` or the user `~/.claude.json`. The named
entry must match a supported Mastermind stdio launcher: the installed binary,
the project/global `mastermind` launcher, or canonical `npx -y
@xcraftmind/mastermind[@version] serve`. Arbitrary executables and packages do
not count, launcher forms must match the current platform, and `env` must be
absent or empty so it cannot replace the executable or npm behavior.

Stable runtime and wiring codes include `layout_ambiguous`,
`mmcg_server_scope_missing`, `mmcg_registration_missing`,
`mcp_registration_entry_invalid`, `mmcg_wildcard_grant`, `mmcg_tool_unknown`,
`mmcg_prompt_grant_missing`,
`model_unsupported`, `effort_invalid`, `max_turns_invalid`,
`tool_allowlist_invalid`, `required_skill_missing`,
`readonly_mutation_capability`, `artifact_definition_conflict`,
`workflow_declaration_limit_exceeded`, and `writer_conflict`. Informational
`tool_grant_unreferenced`, `tools_unreachable`, and `role_unconditional`
diagnostics do not claim that an unmentioned grant is wrong or that a role is
automatically invoked.

Each context estimate uses `ceil(UTF-8 bytes / 4)`. Agent bodies, advisory
skills if loaded, and known mmcg tool schemas are separate scenarios. Missing
optional skills and built-in tools whose schema is unavailable are named, and
no field represents a guaranteed runtime total.

### Incremental indexing — how it works

When you run `mmcg index`, mmcg compares each file's filesystem mtime against the mtime stored in the index:

- **untracked and ignored by Git or `.ignore` rules** → do not scan; tracked files remain candidates even when a broad ignore rule matches them
- **mtime differs from stored in either direction** → re-parse and commit (counted as `indexed`)
- **mtime equals stored** → skip without parsing (counted as `unchanged`)
- **file in index but not on disk** → purge from index (counted as `purged`)
- **binary-looking or larger than 5 MiB** → skip safely and report the count plus a bounded path sample
- **unsupported extension** → skip and report the count plus a bounded path sample

The database also stores extractor and concept-corpus contract versions. When
parser, extractor, or concept-normalization semantics change, the next ordinary
`index` run automatically performs the required rebuild even when file mtimes
are unchanged. `status`, `doctor`, and `mmcg_status` expose contract drift so
agents do not trust stale derived data. `mastermind status` also checks the live
durable-history inventory under the same ten-second deadline as source
freshness. It reports the index as up to date only when all four checks pass.
`mastermind doctor` uses that same scan for its `index freshness` check and
returns a warning if any structural, concept, or durable-history input is stale,
incomplete, changing, or cannot be checked within the bound.

Output example:

```
indexed 3 (unchanged 124, purged 1, skipped binary 2, skipped large 1, failed 0) / scanned 1247 | 87 symbols | 412 edges | 87 concept rows (orphans purged 0, contract rebuilt false) | 4 task specs | 84 history entries (skipped 0, truncated false) | 84 ms
```

When to use `--force`:
- After a schema version change (schema and extractor-contract mismatches already rebuild automatically, but `--force` lets you request a cold rebuild explicitly)
- If you suspect a writer changed content while preserving the exact stored mtime
- For benchmarking — to see how long a cold index takes

**Index-root binding.** The database records the canonical project root on its
first index. A later attempt to reuse that database with a different root is
rejected before mutation, so orphan cleanup cannot purge files merely because
the caller switched scope. Use a separate `--index` path for another root.
`mmcg watch` keeps the root supplied at startup.

The index lives at `.mastermind/mmcg.db` in the current directory by default. Override with `--index <path>` or env var `MMCG_INDEX_PATH`.

## Optional SCIP semantic overlay

Tree-sitter remains the default graph: it is local, incremental, portable, and
does not require every language toolchain. `mmcg enrich --scip index.scip`
decodes the standard SCIP protobuf and atomically replaces only a separate set
of `semantic_*` tables. It never rewrites `symbols`, `edges`, files, history,
or scratchpad data. A failed import leaves the previous overlay intact.

The importer accepts typed SCIP ranges and the deprecated packed range form,
validates canonical repository-relative document paths, rejects paths and
symlinks that escape the indexed root, and bounds the artifact at 512 MiB,
documents at 500,000, occurrences at 10 million, definitions at 2 million,
symbol-information records at 2 million, and semantic edges at 5 million. It
copies the selected regular file once through a no-follow parent capability,
then streams every top-level protobuf pass from that same private snapshot
instead of retaining the full SCIP index and all embedded source text in memory.
The snapshot and original artifact identity and digest are rechecked before the
overlay is replaced. Source files are read through one retained repository-root
capability and hashed at import. When a document embeds source text, its digest
and text comparison use the same bounded read; textless documents are hashed as
bounded streams. Before replacement, the importer revalidates every source
path, file identity, size, and digest. The reported `project_root` must resolve
to the indexed repository; a
portable or moved artifact is accepted only when every `Document.text` exactly
matches its current file. Successful sources expose `repository_verified`.
When some text is omitted, repository identity is still verified but the result
carries `semantic_artifact_revision_unverified` and remains `partial`. Later
file changes suppress affected semantic facts as stale rather than silently
mixing revisions. Definition and edge collections count those suppressed rows
as `omitted_stale`, set `truncated`, and keep an exact stored-match `total` when
the query limit itself was not reached. Unverified repository identity or an
unreadable overlay leaves collection totals unknown instead of presenting the
withheld evidence as a measured empty result.

SCIP occurrence references become `reference` or `import` evidence; explicit
SCIP relationships remain `implementation`, `type_definition`, `definition`,
or `reference`. These are not relabelled as calls because SCIP occurrences do
not prove that every reference is a call. The resolution contract is explicit:

- Tree-sitter topology is the default and the no-SCIP fallback;
- an exact matching SCIP symbol/file endpoint is the preferred static source,
  with `provenance=scip` and `confidence=high`;
- a Tree-sitter-only edge stays `syntactic` / medium confidence;
- OpenTelemetry remains observed runtime corroboration and never creates
  topology.

Use `mmcg query semantic SYMBOL` or the non-destructive `mmcg_semantic` MCP tool
to inspect definitions and relationships directly. The MCP call may refresh the
managed derived index first. Lens loads a valid imported
overlay automatically, shows its producer and precision diagnostics, and only
decorates exact returned endpoint and display-name pairs. `mmcg map`, callers,
impact, and all existing tools continue to work unchanged without SCIP.

## Declarative fact-ingestion SDK

`mmcg enrich --facts MANIFEST` accepts the public strict
[`mastermind-facts/v1` schema](../../schemas/mastermind-facts-v1.schema.json).
First run `mmcg query facts --top 1` to obtain the current API version,
capability list, repository identity, and exact Git revision. Producers may
declare bounded `annotations` and `relationships`; they must bind every source
and provenance artifact to a canonical repository-relative path, byte size,
and SHA-256 digest.

`mmcg facts adapt` creates that manifest directly from one bounded SARIF,
LCOV/Cobertura, JUnit, or OTLP JSON artifact. It uses the same parsers as Lens,
requires every parsed fact to map to the current index, and publishes nothing
when parsing is partial or truncated. Exact duplicate findings are collapsed by
their content-derived IDs. `mmcg facts keygen` creates a non-overwriting local
Ed25519 keypair from the operating system CSPRNG; `mmcg facts sign` and
`mmcg facts verify` add a domain-separated Ed25519 proof defined by
[`mastermind-fact-signature-v1`](../../schemas/mastermind-fact-signature-v1.schema.json).
Trusted import uses `mmcg enrich --facts ... --signature ... --public-key ...
--trusted-key-id ... --require-signature`; revocation IDs override the trust
allowlist. Lens, CLI, MCP, and review export preserve the verified key ID and
reproducible proof. Signatures prove allowlisted key control, not signer
identity or signing time.

Mastermind validates the complete manifest before an atomic per-producer,
per-dataset replacement. Unknown/duplicate fields, unsupported capabilities,
path traversal, symlinks, digest/size mismatch, repository/revision mismatch,
or stale codegraph files fail before the previous dataset is touched. Later
source or revision drift makes the source explicitly stale and suppresses its
facts. Import is capped at 16 MiB per manifest, 10,000 referenced files and 512
MiB of source bytes, 64 provenance artifacts and 256 MiB of artifact bytes, and
100,000 facts.

`mmcg query facts --path PATH --top N` and the fixed read-only `mmcg_facts` MCP
tool expose the same normalized snapshot, bounded provenance-artifact digests,
and explicit limits. Lens projects
annotations as source-labelled findings. A relationship may decorate an
existing returned codegraph edge only when both file and line endpoints match;
it cannot create topology.

The manifest is data only. It cannot load native code, register MCP handlers,
define executable policy rules, modify the language registry or graph, install
a Tree-sitter grammar, execute a command, or access SQLite. Only Mastermind
writes the private normalized tables. See the complete
[producer guide and security model](../fact-ingestion-sdk.md).

## Local team graph

`mmcg team lock MANIFEST --output LOCK` resolves up to 16 local repository
roots and indexes, then pins each credential-free identity, Git revision, and
exact database plus active-WAL digest. Its JSON summary includes the exact
`manifest_sha256` required by MCP authorization. Repository IDs use only ASCII
letters, digits, dots, underscores, and hyphens so namespaced node IDs cannot
collide. `mmcg team map LOCK` reopens those
indexes through the read-only snapshot path, proves source/index freshness, and
returns a bounded repository-namespaced graph. Internal imports remain
Tree-sitter evidence; cross-repository edges are only explicit
`team-manifest` claims and are never inferred. Database and WAL hashing retains
one parent-directory capability per repository inspection and rejects path
substitution, symlinks, and special files while checking identity before and
after each bounded stream.

The fixed read-only `mmcg_team_map` MCP tool accepts a locked manifest inside
the repository served by that MCP process only when the server operator has
authorized that exact path with `MMCG_TEAM_MANIFEST`. Identity, revision,
and exact manifest bytes must also match `MMCG_TEAM_MANIFEST_SHA256`. DB/WAL
drift, duplicate canonical repositories, bounds, and timeouts fail closed. See
the full [`mastermind-team/v1` contract](../team-graph.md).

## Temporal Graph (`mmcg temporal`)

`mmcg temporal --since REF` compares the indexed working copy with a resolved
Git commit without checking out the old tree. Mastermind clones the exact
current SQLite connection snapshot into a private temporary writable database,
batch-loads the changed blobs at the baseline, rewinds only supported changed
source paths, and runs the same bounded `project_map` engine on both sides.
The repository database and WAL bytes, source files, and Git index remain
unchanged. SQLite may update reader-coordination marks in an existing SHM file
while taking a consistent active-WAL snapshot. Data-version plus source
database/WAL metadata are rechecked so a concurrent watcher cannot mix two
SQLite revisions into one result. A fully deleted selected scope is
reconstructed from the baseline instead of failing as an empty head map.

Schema v1 reports:

- added, removed, and file/language-drifted components;
- added, removed, and signature-drifted cross-component boundaries. The same
  evidence is exposed as `public_api`; it is observed external graph use, not a
  claim about a language's declared visibility;
- introduced, resolved, and membership-changed dependency cycles. Expanding or
  merging an existing SCC is not mislabeled as a newly introduced cycle;
- in-degree and rank movement for symbols observed in both bounded hotspot
  windows, plus entries/exits from those windows;
- CODEOWNERS changes across the bounded selected source paths, changed files,
  and returned boundary paths, using the base and head files independently with
  last-match-wins semantics. This includes ownership-only rule changes;
- review candidates when current indexed history exactly mentions a deleted
  path, removed component, removed public boundary, or signature-drifted public
  API. This is a correlation signal, not proof that an ADR or decision is
  obsolete.

The file change set is capped at 10,000 and must be complete before rewind; an
overflow fails closed. Output sections have their own limits, and the response
sets `partial: true` when either project-map projection or a temporal section is
truncated. Each delta collection keeps an exact `total` when its source
projection is complete, including when only the returned page is bounded. A
partial base/head map, unavailable or bounded CODEOWNERS input, or incomplete
history corpus sets the affected `total` and summary count to `null`. An
observed architecture change remains `true`; when no change is observed but a
required source is incomplete, `summary.architecture_changed` is `null` rather
than a false unchanged conclusion. `components_changed` preserves component
file/language drift in every summary surface. Ownership checks at most 500
relevant paths; history scans at most 5,000 derived artifacts and 32 MiB and
returns at most 500 candidates. Diagnostics are capped at 100
with an explicit `diagnostics_truncated` flag. Git, CODEOWNERS, history, rewind,
and SQLite phases cooperatively observe the request budget/cancel signal. The
head CODEOWNERS file is a bounded regular-file read through a retained,
no-follow parent capability, so special files and path swaps fail closed.
Temporal topology is Tree-sitter syntactic evidence in v1; SCIP, runtime,
coverage, and test overlays keep their separate provenance in Lens.

The same response is available through non-destructive `mmcg_temporal`, which
may refresh the managed derived index first, and appears in Lens below the blast
trace. Lens renders bounded identities for component,
boundary/API, cycle, hotspot, ownership, and history events. It isolates a
bounded temporal-unavailable result so ordinary impact evidence remains usable,
but a repository, Git, or SQLite snapshot race still fails the refresh.

## Mastermind Lens (`mmcg ui`)

Lens is a browser review surface for one baseline-to-working-tree change. It
does not maintain a second analysis engine: `/api/lens` wraps the existing
schema-v1 project-map and change-impact responses, including their truncation,
precision, collision, and work-limit notes.

If the shared changed-file collection has already hit its 10,000-file cap,
Lens serializes at most 200 of those file records. The Lens-only collection
fields keep the distinction explicit: `returned` is the displayed item count,
`observed` is the impact engine's lower bound, and `projection_truncated` with
`projection_reason: lens_payload_limit` identifies the transport projection.
Impact analysis, SARIF, summaries, and manifests continue to use the full
bounded collection in memory. A changed path that cannot be represented as
UTF-8 is omitted, makes the source collection partial with
`truncation_reason: non_utf8_path`, and adds a counted
`non_utf8_changed_paths_skipped:N` precision note. When the file cap also
applies, `file_limit` remains the collection reason and the precision note
preserves the separate omission count.

The server binds only to `127.0.0.1`; port `0` is the default and lets the OS
choose a free port. It accepts same-origin `GET`/`HEAD` requests, serves embedded
offline assets under a restrictive content-security policy, and opens the
existing SQLite index in query-only mode. A checkpointed index is opened
directly as immutable; an active WAL is copied with the database into a bounded
private temporary snapshot (2 GiB and at most 60 seconds, or the shorter request
deadline) through SQLite's online backup API. Database and WAL bytes remain
unchanged; SQLite may create or update the SHM reader-coordination file required
for a consistent active-WAL read.
Refreshes fail closed when the repository, index, WAL, baseline, or work
snapshot changes, or when indexed source files disappeared. The final check
runs after evidence, audit, and the optional document graph, and binds the exact
bounded working-tree projection plus its omission state. There are no
source-content or mutation routes.

`--since` is required. `--path`, `--depth 1..5`, `--top 1..100`, and
`--production-only` bound the initial review. Run `mmcg index .` first; Lens
will report a missing or stale index rather than create or update one.

### Selected-scope audit

The audit view is a deterministic projection of the same selected map scope,
not a separate whole-repository scan. Component structure, dead-code
candidates, centrality, largest-file line-span proxies, Git churn, and
authorship concentration all apply `--path` and `--production-only` in the
backend before their result caps. The browser displays that fixed policy and
does not reclassify or post-filter capped results. Dead-code, centrality,
change-hotspot, largest-file, and authorship windows expose truncation instead
of presenting a capped page as complete.

SQLite or Git failures stay explicit. A failed audit query is reported as
unavailable and cannot produce a `Healthy` or `Clear` presentation. Component
map truncation is also visible; omitted components are not represented by the
visual `Other returned components` tile. Static findings in the security card
remain change-scoped evidence because evidence overlays are correlated only to
the returned change, impact, and candidate-test trace. They are not a
repository-wide clean result.

### Optional AI audit narrative

mmcg never invokes a model. It can read an optional bounded interpretation from
`.mastermind/audit-narrative.json`, or from the path in
`MMCG_AUDIT_NARRATIVE`. Relative overrides are repository-relative; absolute
overrides must still resolve inside the selected repository. The sidecar is
opened through the same no-follow capability and 256 KiB/deadline limit as
other repository-owned inputs. Start from `audit.narrative_binding` in the current
`/api/lens` response and copy that object unchanged into the sidecar's required
`binding` field. The binding covers repository identity, baseline and HEAD,
the working-tree snapshot, and the exact returned map. A stale or foreign
binding is rejected and Lens falls back to facts-only output. The default
sidecar path is treated as Mastermind runtime state, so writing it does not
invalidate the snapshot it describes.

Validate producers against
[`schemas/mastermind-audit-narrative-v1.schema.json`](../../schemas/mastermind-audit-narrative-v1.schema.json).
Domain component lists and red-team routes must contain only exact component
paths returned by the bound map; unknown or omitted components are rejected.
The UI labels narrative prose as AI interpretation and routes as claims to
verify. Neither text nor a straight line between component tiles is topology
proof, runtime evidence, or a confirmed vulnerability.

### Evidence overlays

Lens can correlate the returned change/impact trace with additional read-only
evidence:

- repeatable `--sarif PATH` inputs for SARIF 2.1 findings;
- repeatable `--coverage PATH` inputs, auto-detected as LCOV tracefiles or
  Cobertura XML;
- repeatable `--junit PATH` inputs for JUnit XML. Only explicit testcase `file`
  attributes are correlated; class names are not guessed into paths;
- repeatable `--otel PATH` inputs for OTLP JSON. Only explicit
  `code.file.path` or legacy `code.filepath` span attributes are correlated;
- exact repository-path mentions from the indexed project-history corpus,
  including specs, executor reports, audits, lessons, context, release notes,
  and Markdown decisions under conventional `docs/adr`, `docs/adrs`,
  `docs/decisions`, `adr`, `adrs`, or `.mastermind/decisions` directories.
  Lens verifies the bounded live Markdown inventory before correlating indexed
  excerpts. Changed or deleted documents suppress history matches with a
  `project_history_stale` diagnostic; incomplete admission suppresses them with
  `project_history_incomplete`. Healthy code and other evidence overlays remain
  available. This is enabled by default; `--no-project-knowledge` disables it;
- one explicit portable document graph with `--document-graph PATH`. Lens reads
  only a root-contained packet under `.mastermind/research`, rechecks its named
  endpoints and optional Markdown corpus, and displays a separate relation
  review queue. `needs_review` is a partial evidence state. `current` describes
  matching bytes only; every relation remains `unverified`;
- CODEOWNERS from `.github/CODEOWNERS`, repository-root `CODEOWNERS`, or
  `docs/CODEOWNERS` in that order, with `--codeowners PATH` as an override;
- bounded Git churn and contributor names from the last 200 commits by default,
  configurable with `--git-commits 0..1000` (`0` disables Git history).
- the repository's persisted SCIP overlay, when present and fresh. It is not a
  UI flag or report path; import it first with `mmcg enrich --scip index.scip`.
- current normalized declarative facts, when present. Import them first with
  `mmcg enrich --facts MANIFEST`; stale repository, revision, or source bindings
  are omitted with a diagnostic.

Evidence is matched only to files already returned by the bounded change,
impact, and candidate-test trace. The versioned `evidence` response includes
source status, file-level facts, diagnostics, and applied limits. A source that
is missing, changes during the read, exceeds 32 MiB, has invalid syntax, hits a
work cap, or exceeds the request deadline is reported as partial/error; it is
never silently treated as a clean result. Findings are capped at 5,000 total
and 100 per file, coverage at 500,000 unique lines, JUnit at 100,000 cases and
1,000 returned failure details, OTLP at 100,000 matched spans and 1,000 file
pairs, project knowledge at 500 exact matches, and combined artifact inputs at
64. CODEOWNERS stays below GitHub's 3 MiB limit and at 50,000 rules and 50
owners per rule, contributor details at five recent names per file, and
diagnostics at 100. Churn totals stay complete when contributor names are
truncated. Git output is capped at 8 MiB.

Explicit evidence paths are resolved once and then read through a retained
parent capability with no-follow handles. Only bounded regular files are
accepted; path substitutions and special files fail closed.

When the changed-file inventory is already truncated, evidence selects symbol,
impact, and candidate-test paths first, then admits at most 200 file-only paths.
The response remains partial and reports `relevant_file_limit`; the cap cannot
be mistaken for complete evidence.

Repository-relative artifact paths match exactly. Reports produced under a
different absolute build root may use a unique repository-path suffix match;
this relocation and the maximum-hit merge used for duplicate coverage lines
are reported as precision notes. Artifact labels preserve provenance, but Lens
alone cannot prove that a SARIF, coverage, JUnit, or OTLP report was produced
from the current Git revision. A PR evidence package binds the exact report
bytes a reviewer saw to its resolved HEAD, while an optional producer
attestation records the stronger revision claim described below. CODEOWNERS
matching uses the working-tree file,
including last-match-wins and explicit no-owner rules; it does not verify GitHub
account/team existence or write permission, and GitHub review assignment still
uses the base-branch file. Git history is pinned to the impact snapshot's HEAD
and does not follow renames.

The UI renders redundant text and visual marks for SARIF, coverage, JUnit,
SCIP, normalized declarative facts, runtime spans, project knowledge,
ownership, and churn. Exact SCIP
symbol/file endpoint pairs supersede Tree-sitter as static provenance without
changing the returned graph. Runtime parent-child
file pairs may decorate an already returned static edge in either direction,
but they never create a node or edge. Overlay switches change emphasis and
inspector detail only; they do not add or remove codegraph topology. Imported
artifacts and Git history are parsed in memory. Project knowledge is read from
the derived SQLite history corpus after its live inventory check; Markdown
remains authoritative and must be re-indexed after changes. Watch mode refreshes
history for nested ADR edits, additions, removals, and renames under the same
decision directories admitted by the indexer. Lens writes none of this evidence
to source files or SQLite.

The portable [project-history skill](../../skills/workflow/mastermind-project-history/SKILL.md)
ships the producer and standalone checker for this packet. The Lens projection
keeps it separate from inferred history candidates, Lens topology, and the
code-file endpoints required by `mastermind-facts/v1`. A different packet Git
revision is shown separately from live content freshness: matching named bytes
do not cover unrelated changes. Relation labels and unchanged hashes do not
establish semantic correctness.

### PR evidence package (`mmcg review export`)

`mmcg review export --since REF --out DIR` captures the same fail-closed Lens
snapshot without starting an HTTP server. `DIR` must not already exist. The
export is assembled in a private sibling temporary directory and renamed only
after every payload is synced, so a failed run does not publish a half-package.

The package contains:

- `index.html`: one autonomous Lens document with the snapshot, CSS, and JS
  embedded under a hash-only CSP. It has no fetch, CDN, telemetry, or write
  path;
- `mastermind.sarif`: the project-map and change-impact SARIF projections as
  two independently identified runs;
- `summary.md`: a short, bounded reviewer summary with links to the HTML and
  SARIF payloads;
- `manifest.json`: strict schema-v1 repository, scope, evidence, payload digest,
  and partial/truncation state bindings;
- `mastermind-review.yml`: the pinned
  [GitHub Actions example](../examples/mastermind-review-pr.yml) for artifact
  and SARIF upload.

The workflow builds the checked-out binary when it runs in the Mastermind
source repository, so a command introduced by the pull request is exercised
before release. In consuming repositories it installs the exact npm version
recorded in the template and verifies `review export` before indexing.
Compilation and analysis run with a read-only token; a separate action-only job
receives `security-events: write` and uploads the already-produced SARIF
artifact.

The export accepts the same `--path`, `--depth`, `--top`,
`--production-only`, `--sarif`, `--coverage`, `--junit`, `--otel`,
`--codeowners`, `--git-commits`, `--no-project-knowledge`, and
`--document-graph` inputs as Lens.
It reads external files before and after analysis, then rechecks them and the
optional attestation after the private package directory is fully written and
synced. The manifest records their SHA-256 digests next to the resolved head
OID. This is a **digest binding at export time**: it proves exactly which report
bytes a reviewer saw at that revision, not that Semgrep, CodeQL, a test runner,
or an OTel collector produced those bytes from that revision.
Each read uses a capability-scoped, no-follow handle and accepts only a bounded
regular file. A path swap, symlink change, or special file fails closed.

The exporter retains the exact read-only SQLite snapshot used by Lens until
publication. At the same final boundary it revalidates the Git HEAD, bounded
worktree projection and omissions, project-history inventory, repository root,
and source database/WAL identity. It then checks the retained staging directory
contains only the declared package files and rereads every payload and manifest
byte through no-follow handles before the atomic rename. A race removes the
private staging directory and publishes no output. When staging is inside the
repository, the worktree check excludes only that exact newly created private
directory; sibling and pre-existing changes remain part of validation.

With `--document-graph PATH`, the manifest also binds the packet digest, its
internal snapshot digest, the stable live observation digest, snapshot Git
metadata, whether that snapshot head matches the review head, content-change
counts, corpus status, and relation count. The embedded Lens and Markdown
summary carry the same check. `needs_review` makes `analysis.partial` true, while
`current` still leaves every relation semantically `unverified`. Export rechecks
the complete graph observation immediately before atomic publication and fails
if the packet or observed bytes changed. It also rejects `--out` beneath a
tracked corpus directory because publishing `summary.md` there would make the
new package stale at creation time.

Loaded `mastermind-facts/v1` datasets need no second sidecar attestation: their
ingestion contract already verified exact repository identity, Git head, source
digests, and provenance-artifact digests before Mastermind wrote normalized
facts. Unsigned datasets remain `producer-attested`. A dataset imported through
an allowlisted Ed25519 key is `producer-signed`; the package records its key ID,
public key, signature, detached-signature digest, and canonical signed-manifest
digest. Mixed evidence is `partially-producer-signed`. The autonomous Lens HTML
contains the same normalized facts. A stale, invalid-proof, or truncated source
remains an explicit partial state and is never promoted to a revision-bound
claim.

For the stronger producer claim, create a strict JSON attestation and pass
`--evidence-attestation PATH`:

```json
{
  "schema_version": 1,
  "head_oid": "0123456789abcdef0123456789abcdef01234567",
  "artifacts": [
    {
      "kind": "sarif",
      "path": "reports/semgrep.sarif",
      "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
    }
  ]
}
```

Every path must be canonical and repository-relative; every listed digest must
match a requested evidence input and `head_oid` must match the Lens snapshot.
Unknown or duplicate JSON fields, digest drift, and revision drift fail closed.
This v1 attestation is explicitly recorded as unsigned CI evidence. A signed
workflow, artifact attestation, or other trust anchor may authenticate its
producer separately. The formal contracts are
[`mastermind-review-manifest-v1.schema.json`](../../schemas/mastermind-review-manifest-v1.schema.json)
and
[`mastermind-evidence-attestation-v1.schema.json`](../../schemas/mastermind-evidence-attestation-v1.schema.json).

### SARIF export

`mmcg map --format sarif` exports returned dependency cycles with the stable
rule ID `mastermind/dependency-cycle`. `mmcg impact --format sarif` exports
returned cross-component change impacts with
`mastermind/component-boundary-change`, anchoring the primary location to the
changed symbol and linking the impacted symbol as a related location. Both are
SARIF 2.1.0 documents with repository-relative, percent-encoded artifact URIs,
Mastermind's semantic version, and run properties that expose query scope,
baseline/head identity, and partial-result state. The exporter never converts a
truncated query into a completeness claim.

Use GitHub's `github/codeql-action/upload-sarif` action when uploading these
files so GitHub can populate missing fingerprints. Stable `ruleId` values and
consistent relative paths are retained across runs for alert identity.
See [GitHub's SARIF support contract](https://docs.github.com/en/code-security/reference/code-scanning/sarif-files/sarif-support)
for ingestion limits and PR annotation behavior.

## Architecture policy as code (`mmcg policy check`)

`mmcg policy check --since REF` evaluates a repository-owned
`mastermind-policy.yml` over one normalized evidence input. Git/SQLite,
CODEOWNERS, and strict workflow artifacts are collected first; the policy
evaluator itself is deterministic and has no Rego, OPA daemon, network access,
or embedded general-purpose runtime. The config is read as a bounded regular
file through a retained repository capability and is revalidated by path,
digest, and file identity after evidence collection.

```yaml
version: 1
rules:
  - id: domain-must-not-import-infrastructure
    from: src/domain/**
    deny_imports: src/infrastructure/**

  - id: no-new-payment-cycles
    scope: services/payment/**
    max_new_cycles: 0

  - id: public-api-review
    when: api_surface_changed
    require_owner: platform

  - id: payment-blast-budget
    scope: services/payment/**
    max_blast_radius: 25

  - id: payment-needs-tests
    scope: services/payment/**
    require_tests: true

  - id: payment-ownership-boundary
    scope: services/payment/**
    deny_ownership_crossings: true

  - id: critical-payment-workflow
    critical: services/payment/**
    require_workflow: strict
```

The schema is intentionally closed: unknown keys, duplicate IDs, invalid globs,
more than one action in a rule, no-op booleans, and configurations above 100
rules are errors.
The default file is `mastermind-policy.yml`; use `--config PATH` to select a
different file inside the repository. A symlink that resolves outside the
repository is rejected. Paths are repository-relative glob patterns with `/`
separators.

Evaluation is diff-first:

- `deny_imports` reports current forbidden import edges only when the source
  file is part of the baseline-to-working-tree change;
- `max_new_cycles` compares current SCC membership with the exact baseline Git
  blobs, without checking out or mutating the baseline worktree;
- `api_surface_changed` means an observed changed symbol reaches another
  inferred top-level component. It is empirical cross-component usage, not a
  language-level `public`/`export` declaration claim;
- `max_blast_radius` counts unique returned impacted symbols whose changed seed
  matches the rule scope;
- `require_tests` accepts bounded graph-linked candidates or test files inside
  the configured scope. A test elsewhere under the same top-level directory is
  not treated as related. The rule does not claim that those tests ran or
  passed;
- `deny_ownership_crossings` compares CODEOWNER sets on the changed and
  impacted sides of an observed component crossing. Missing ownership makes
  the rule incomplete rather than silently clean;
- `require_workflow: strict` requires a canonical strict `spec.md`, matching
  baseline `state.json`, held `audit.md`, and an exact touched-file entry. The
  held state carries a SHA-256 snapshot of every declared touch, so editing a
  critical file after the audit invalidates the evidence.

New held states use `held_snapshot_version: 2`. The digest binds the exact
baseline commit, normalized touch paths, file bytes, missing files and effective
Git executable modes. Staging or committing the same files preserves it, including
in a separate CI checkout with the same bytes and modes. Line-ending conversion
or clean/smudge filters that change those bytes require another audit. With
`core.filemode=false`, the index supplies executable modes; untracked files start
as `100644`, matching Git. Present symlinks, Git symlink placeholders and submodules
are not supported as regular-file evidence.

States without a version retain the original v1 digest rules, including their
sensitivity to staging. Re-run post-flight on the intended final files to issue
a v2 snapshot; an existing semantic review may need renewal. Unsupported versions
fail closed. A failed v2 comparison never falls back to v1.

The default workflow evidence directory is `.mastermind/tasks`; CI must restore
the canonical task artifacts or point `--workflow-evidence PATH` at the
downloaded evidence directory. An absolute directory outside the checkout is
supported. Its layout is `<directory>/<task-id>/{spec.md,state.json,audit.md}`;
Action `*.bundle.json` files are a different format. Reads use one directory
capability, reject symlink components below it and recheck the complete read set
after repository/index validation. Unreadable artifacts make evaluation incomplete
even when another task supplies coverage. CODEOWNERS is discovered in GitHub
priority order, or overridden with `--codeowners PATH`.

Policy topology stays on the fast default syntactic graph in v1. Imported SCIP
and runtime overlays retain their provenance for Lens but do not add or remove
policy edges. Strict workflow evidence validates canonical local artifact
consistency; it does not prove an audit signature, GitHub approval, or remote CI
execution.

Impact defaults are depth 3 and top 500 for policy checks. Override them with
`--depth 1..5` and `--top 1..500`. Import/cycle work is capped at 50,000 file
edges; baseline cycle comparison is capped at 500 files and 32 MiB; workflow
evidence is capped at 1,000 directory entries, 1 MiB per artifact and 32 MiB total
per read pass (including specs for unrelated tasks). A strict snapshot can
bind at most 1,000 touch files, 16 MiB each and 32 MiB total. Reports stop at
1,000 total results (reserving one result for the fail-closed diagnostic) and
become incomplete rather than allocating unbounded SARIF. Any relevant cap, stale index, concurrent snapshot
change, or unreadable required evidence exits non-zero. JSON and SARIF preserve
`complete`, diagnostics, baseline/head OIDs, config path, and config SHA-256.
SARIF uses each configured rule ID as its stable `ruleId`; incomplete evaluation emits
`mastermind/policy-evaluation-incomplete` at the config location.
Policy results also include deterministic `partialFingerprints` so repeated
uploads can retain alert identity when source line numbers move.

See [`docs/examples/mastermind-policy.yml`](../examples/mastermind-policy.yml)
for all v1 rule forms.

## MCP server usage

```bash
mmcg serve
```

### Protocol contract

- Supported revisions are MCP `2025-11-25` and legacy `2024-11-05`. Unknown requested revisions receive the latest supported revision; clients that cannot support it disconnect.
- Current results include compact JSON text and object `structuredContent`. Scratchpad reads keep their JSON-array text form and are wrapped as `{ "entries": [...] }` only in structured form. Legacy results remain content-only.
- Tool execution and input errors use `isError: true`; malformed protocol requests and internal failures use JSON-RPC errors.
- Input frames are limited to 1 MiB and an oversized frame closes the connection. The complete serialized JSON-RPC response is kept within 8 MiB (including duplicated text/structured content and envelope reserve); oversized results ask the caller to narrow the query.
- Tool annotations are advisory metadata, and returned tool content is untrusted. Neither grants permission or bypasses confirmation.
- Every tool dispatch runs under a work budget and can be interrupted by a client cancel notification — see [Work budgets, timeouts, and cancellation](#work-budgets-timeouts-and-cancellation).

Use the dry-run-first setup surface instead of editing supported client configs by hand:

```text
mastermind setup <claude|cursor|codex|continue|generic> \
  --scope <project|user> [--root .] [--config PATH] [--write] [--remove] [--force]
```

Claude supports project JSON and user-native registration; Cursor supports project and user JSON; Codex is user-only through its native CLI; Continue owns a standalone `mastermind.yaml`; Generic requires `--config`. `--force` permits customized replacement/removal but never implies `--write`; file-backed customized data is backed up privately under `~/.mastermind/setup-backups/`. Doctor compares bounded config data to the trusted current binary and never executes configured commands.

The equivalent generic MCP JSON shape is:

```json
{
  "mcpServers": {
    "mmcg": {
      "command": "mmcg",
      "args": ["serve"],
      "env": {
        "MMCG_INDEX_PATH": "/absolute/path/to/your/project/.mastermind/mmcg.db"
      }
    }
  }
}
```

Run `mmcg watch` in a separate terminal so the index stays current while you work.

Structural MCP queries also refresh the managed `.mastermind/mmcg.db` on demand
when source files or the extractor contract drift. The repository root is
derived from the canonical database path and checked against the stored index
identity before any refresh. Automatic refresh admits at most 20,000 source
candidates and 512 MiB of declared source bytes; exceeding either cap returns
`refresh_limit_exceeded` without a partial refresh. A custom external `--index`
is opened read-only by `serve`: it remains query-compatible when fresh, requires
an explicit `mmcg index` when stale, and is never created, migrated, truncated,
or given a WAL by the server. Reading an existing active WAL may create or
update its SHM coordination file. Incompatible custom schemas return
`schema_incompatible`. Other failed or unavailable refreshes return
`index_stale`.

## MCP tools

| Tool | Args | What it returns |
|---|---|---|
| `mmcg_search` | `name`, optional `kind`, `language`, `collapse_partials` (default `true`), `top` (default 100, max 200) | Symbols matching exactly. MCP results report effective `total`, returned `count`, truncation, and raw-candidate coverage. With partial collapsing, at most 500 raw declarations are grouped; if that work cap is reached, `total` is null and `truncation_reason` is `raw_work_limit` because partial groups and location arrays may be incomplete. Disable collapsing for an exact raw total. The CLI retains its complete local listing. |
| `mmcg_callers` | `name`, optional `language`, `edge_kind` (default `calls`), `top` (default 100, max 500) | **Containing functions** that reference `name` by the given edge kind. MCP responses report exact `total`, returned `count`, `truncated`, and `row_limit`; add language when truncated. The CLI retains its complete local listing. Count = distinct containing units, not distinct call sites (a function with 3 calls to `name` counts once). Pass `edge_kind: imports` for importers or `references` for function-value and Rust macro-body usages. Returns the effective filters, per-symbol precision, and `precision_notes`; references are not proof of invocation. |
| `mmcg_callees` | `name`, optional `language`, `edge_kind` (default `calls`), `file`, `line` (requires `file`), `top` (default 100, max 500) | Outgoing names from one definition. `match_status` is `matched`, `ambiguous`, or `not_found`; ambiguous results contain bounded `candidates` and no selected edges. `candidate_total` and `candidates_truncated` describe definition selection, while `total`, `count`, and `truncated` describe outgoing edges. A precise `file`/`line` selector is queried independently, so a broad candidate cap cannot hide the requested definition. The CLI retains complete output. `name_collision` counts exact-name definitions after the language filter, before location selection. Partial-class declarations remain separate candidates. |
| `mmcg_impact` | `name`, optional `max_depth` (1-10, default 2), `language` | Transitive dependency candidates through `calls` and syntactic `references` edges. Responses echo the effective language, report exact filtered `total`, returned `count`, and `truncated` at the 5,000-row result cap. The language filter scopes seed definitions and every walk step; `name_collision` uses that same scope. Narrow `max_depth` or add a language when truncated. `precision_notes` remain present on empty results; complete indexed coverage does not prove complete runtime reachability. |
| `mmcg_imports` | `file`, optional `top` (default 200, max 500) | Indexed static imports declared by the file; each entry has `name`, fully-qualified `path` when extracted, and `line`. MCP responses report exact `total`, returned `count`, `truncated`, `row_limit`, and precision notes for missing dynamic imports. `mmcg query imports` retains the complete local listing. |
| `mmcg_imported_by` | `query`, optional `match: name`(default)/`path`, `language`, `top` (default 200, max 500) | Files whose indexed static imports reference the given name or fully-qualified path. The response echoes the selector and language, reports exact `total`, returned `count`, `truncated`, `row_limit`, and precision notes. Use `match: path` when a leaf name is ambiguous; narrow by path or language when truncated. The CLI retains its complete local listing. |
| `mmcg_symbols_in_file` | `file`, optional `top` (default 200, max 500) | Syntactically extracted symbols in a file, in deterministic source order. MCP responses report exact `total`, returned `count`, `truncated`, `row_limit`, and extraction precision notes. The CLI retains its complete local listing. |
| `mmcg_outline` | `file`, optional `top` (default 200, max 500) | Parent-preserving symbol tree of a file — classes/impls own their methods, modules own top-level functions. MCP responses bound the total number of nested nodes, report exact `total`, returned `count`, truncation, and a 64-level traversal limit. `depth_limit` or `hierarchy_incomplete` warns that the returned tree is not a complete prefix. The CLI retains its complete local tree. |
| `mmcg_files` | optional `prefix`, `language`, `top` (default 200, max 500) | Indexed files with symbol counts. MCP responses fetch at most `top` rows, echo both effective filters, and expose exact filtered `total`, returned `count`, `truncated`, and `row_limit`; narrow by literal prefix or language when truncated. The CLI file query retains its complete local listing. |
| `mmcg_recent_changes` | `since` (e.g. `2h`, `30m`, `1d`), optional `top` (default 200, max 500) | Indexed file snapshots whose stored source mtime falls between `window_start_unix_ms` and `as_of_unix_ms`. MCP responses report exact `total`, returned `count`, `truncated`, and `row_limit`, identify `timestamp_basis`, and exclude future mtimes. The CLI retains its complete local listing. This is a filesystem recency signal rather than indexing-time or Git evidence. |
| `mmcg_scratchpad_append` | `agent`, `kind`, `body` | Append a one-line intent / note / handoff to the cross-agent scratchpad — live in-session channel between Mastermind subagents (planner → executor → auditor). Persists in `.mastermind/mmcg.db`. Body capped at 8 KiB. Cross-session counterpart is `_lessons.md`. |
| `mmcg_scratchpad_read` | optional `since`, `agent`, `kind`, `limit` | Read recent scratchpad entries, newest first. `since` is a unix timestamp (seconds); omit for the last `limit` entries (default 20, max 200). The response echoes every effective filter and reports exact filtered `total`, returned `count`, `truncated`, and `limit`. |
| `mmcg_change_class` | `file` | Classify a file's last change as `structural`, `cosmetic`, or `first-seen`. Backed by an FNV-1a 64-bit hash of the file's parsed structural shape — line numbers and whitespace excluded. Pre-edit signal for planner and auditor: large diffs that are mostly cosmetic have smaller real scope than line count suggests. |
| `mmcg_unreferenced` | optional `kind`, `language`, `top` (default 100, max 500) | Symbols with no indexed reference candidates, with per-symbol precision and explicit `precision_notes`. MCP responses expose the exact filtered `total`, returned `count`, `truncated`, and `row_limit`; narrow by kind or language when truncated. The CLI retains its complete local listing. These are not proven dead code. **Review manually** — see Limitations for false-positive scenarios. |
| `mmcg_api_surface` | `prefix`, optional `language`, `top` (default 100, max 500) | Symbols under the literal `prefix` with at least one outside syntactic name-and-kind reference candidate. MCP responses expose the exact filtered `total`, returned `count`, `truncated`, `row_limit`, per-symbol edge precision, and response precision notes; narrow the prefix or add language when truncated. The CLI retains its complete local listing. Same-named definitions may be false-positive boundary candidates, so this empirical map is not compiler-resolved identity or declared visibility. |
| `mmcg_centrality` | optional `prefix`, `language`, `kind`, `top` (default 20, max 200) | Rank symbols by in-degree (distinct callers). Responses report the exact filtered `total`, returned `count`, `truncated`, effective filters, and precision notes, so a full `top` page is not mistaken for the whole ranking. Excludes synthetic `<module>` rows and zero-degree symbols. Name-based resolution can pool collisions; ranking is a structural reading-order hint, not runtime importance proof. |
| `mmcg_semantic` | `symbol`, optional `top` (default 100, max 500) | Compiler-resolved SCIP definitions, references, implementations, type definitions, and explicit provenance. Returns `fallback_active: true` instead of an error when no overlay exists. Each collection reports stored-match `total` when exact, returned rows, query/stale truncation, and `omitted_stale`; repository or revision uncertainty keeps the response partial. |
| `mmcg_facts` | optional `path` (default `.`), `top` (default 100, max 400) | Normalized `mastermind-facts/v1` annotations and relationships plus capability negotiation, exact repository/revision identity, provenance, source state, limits, and stale/truncation diagnostics. Read-only; it never loads producer code or creates graph topology. |
| `mmcg_team_map` | `manifest` | Bounded `mastermind-team/v1` graph over pinned local read-only indexes. The locked manifest must be repository-relative, inside the MCP server root, and exactly authorized by `MMCG_TEAM_MANIFEST` plus `MMCG_TEAM_MANIFEST_SHA256`. Nodes are repository-namespaced; internal imports retain Tree-sitter provenance and cross-repository edges are explicit manifest claims. |
| `mmcg_map` | optional `path` (default `.`), `depth` (1–6, default 2), `top` (1–100, default 20), `production_only` (default `false`) | Schema-v1 architecture briefing with lexical file/directory scope: `%` and `_` are literal bytes, selected-directory components are relative to that directory, root components remain repository-relative, and selected files retain their paths. `production_only` excludes conventional test/fixture/example/generated/vendor path segments and test filenames (`test_*`, `*_test.*`, `*.test.*`, `*.spec.*`, `*Test.*`, `*Tests.*`) before bounded queries run. Hotspots prefer unambiguous definitions before pooled same-name collisions. JSON, text, Mermaid, and CLI SARIF are projections of the same result. Text reports per-section coverage and returned cycle members. Mermaid includes entry points, components, boundaries, hotspots, and cycle rings; when its visual summary omits returned rows, a notice names the omitted counts and points to JSON/text. SARIF exports returned cycles and derives its partial metadata from every top-level section and component boundary. Caps are 50,000 aggregation paths, 20 languages, 20 components, 20 boundaries/component and 400 globally, 50 entry points, 100 hotspots, 50,000 scoped cycle edges, 50 cycles, and 500 cycle memberships. Every truncated section names its cause: `path_work_limit` marks path-derived partial aggregates; `language_limit`, `entry_point_limit`, and `top_limit` identify section caps; `top_probe` marks a hotspot or per-component boundary cap+1 probe; `global_probe_limit` marks components whose certainty was prevented by the 401st global boundary row; `cycle_limit`, `cycle_membership_limit`, or `cycle_and_membership_limit` identify bounded complete SCC output; cycle `work_limit` returns no cycles because SCC analysis was skipped before truncated edges could be analyzed. |
| `mmcg_temporal` | `since`, optional `root`, `path` (default `.`), `depth` (1–5, default 2), `top` (1–100, default 20), `production_only`, `codeowners` | Schema-v1 base-vs-indexed-worktree architecture delta. It rewinds changed Git blobs only in a private SQLite snapshot and reports components, public boundaries/API, cycles, centrality/hotspot drift, base/head CODEOWNERS changes, history review candidates, provenance, limits, and partial diagnostics. Collection totals and summary counts are exact only when their source projections are complete; otherwise they are null. A truncated 10,000-file change set fails closed. |
| `mmcg_change_impact` | `since`, optional `root`, `depth` (1–5), `top` (1–500) | Stable schema-v1 analysis of the resolved baseline against staged, unstaged, and untracked content. Reports added/removed/signature/body-changed symbols, batched dependency candidates through calls and references, component crossings, ranked test candidates, a `disciplines` block routing the change to an evidence set, exact collection metadata, caps, and precision notes. Stable file-limit projections remain partial; non-UTF-8 paths are omitted, counted, and mark the file collection partial. Root, SHA-256 index freshness, Git snapshot, and SQLite snapshot checks fail closed with stable codes. |
| `mmcg_brief` | `role` (`planner`, `executor`, or `auditor`), `since`, optional `root`, `budget_tokens` (256–8,000; default 2,000) | One deterministic schema-v1 role packet over the checked worktree, structural graph, and project-history inventory. Role changes prefix admission order, not fields. The accepted budget covers the serialized MCP result after JSON escaping, `content.text`, and `structuredContent` duplication. Repository paths and symbol names are capped, control/bidi-escaped untrusted data; source bodies, signatures, literals/defaults, history titles, and excerpts are excluded. |
| `mmcg_test_impact` | `since`, optional `root`, `depth` (1–5), `top` (1–500) | Exact test-focused projection of `mmcg_change_impact`. Changed tests and depth-1 graph tests are direct, deeper graph tests are transitive, and same-component candidates without graph evidence in this response are heuristic. Explicit supported test attributes identify candidates independently of filename, including inline Rust `test`, `tokio::test`, and `async_std::test`. Name heuristics still require test-like paths; fixtures and lifecycle hooks remain excluded. Fallback evidence is `same_component_test_filename` for test-like paths or `same_component_test_attribute` otherwise. Focused candidates never replace the repository's full required gate. |
| `mmcg_tasks` | `query`, optional `top` (default 10, max 50) | Full-text search canonical task specs (`.mastermind/tasks/<NNN>-<name>/spec.md`) through the same deterministic inventory as `mmcg_history`. FTS5 MATCH syntax accepts bare AND-joined words, `"phrases"`, and `OR`/`NOT`. Returns ranked paths, titles, excerpts, exact indexed-match coverage, page and corpus truncation, skipped-artifact count, and live history `freshness`. A match is retrieval evidence rather than proof of current behavior or an accepted decision; the returned Markdown remains authoritative. Top-level `_` files and bare legacy `.md` files under `tasks/` are excluded. |
| `mmcg_history` | `query`, optional `kind`, `top` (default 10, max 50), `document_graph` | Searches `CONTEXT.md`, `CONTEXT-archive-*.md`, canonical task specs, executor reports, audits, `.mastermind/releases/*.md`, legacy task-local release notes, lessons, and Markdown architecture decisions under conventional ADR directories. `architecture_decision` is an exact `kind` filter. `candidate` lessons are unresolved signals, not active guidance. Returns `indexed_total`, `count`, `result_truncated`, `row_limit`, observed matches, `skipped_artifacts`, `corpus_truncated`, overall `truncated`, `freshness` (`fresh`, `stale`, `incomplete`, or `snapshot_changed`), and an explicit retrieval-only epistemic contract. `indexed_total` is exact only for the admitted FTS corpus; skipped or corpus-truncated Markdown stays outside it. Markdown remains authoritative. The deterministic inventory binds path, kind, length, content digest, skipped state, and truncation state. Limits are 1 MiB per artifact, 5,000 artifacts, and 32 MiB of admitted text. When `document_graph` names a root-contained packet under `.mastermind/research`, the response also includes a separate live, no-follow `document_graph` check. It writes nothing to SQLite and never upgrades `verification: unverified`; its content status is independent of history-index freshness. The CLI equivalents are `mastermind history <query> --document-graph <path>` and `mastermind query history ...`. |
| `mmcg_dependency_cycles` | optional `language`, `min_size` (default 2), `top` (default 50, max 200) | Detect circular imports as strongly-connected components in the file-level import graph. MCP responses return at most 500 file memberships across complete SCC lists; a cycle that cannot fit is omitted rather than returned partially, with `truncation_reason: member_limit`. `total` and `total_members` remain exact when cycle detection ran. The CLI retains every cycle. Work is capped at 50,000 file-pair edges; above that, Tarjan is skipped, totals are null, and `graph_work_limit` marks the result incomplete. Name-based import resolution can over-approximate, so verify before refactoring. |
| `mmcg_symbols_changed_since` | `git_ref`, optional `root`, `top` (default 100, max 500) | Symbol-level diff between a git ref and the current index. The existing flat arrays remain available, while `coverage` reports exact observed totals, returned counts, and per-collection truncation. MCP returns at most `top` items from each of `files_in_diff`, `added`, `removed`, `signature_changed`, and `errors`; the CLI stays complete. Re-parses old blobs using the same extractor. Git subprocesses are time-bounded and the file loop stops at 10,000; when that source cap is reached, complete totals are null and `source_truncated` prevents treating the observed prefix as the full change set. |
| `mmcg_status` | — | Index path, file/symbol counts, separate `extractor_contract_current` and `concept_contract_current` signals, live `history_freshness`, and source freshness from one checked SQLite snapshot. History is reported independently as `fresh`, `stale`, `incomplete`, `snapshot_changed`, or `unknown`; `history_freshness_error` identifies a failed or work-limited scan. The concept flag is false after an interrupted/failed derived-corpus update or a normalization change, even when the structural graph is current. `freshness_basis: path_and_mtime` makes the metadata contract explicit: added, deleted, older, and newer mtimes are stale; content changed while preserving the exact mtime requires `mmcg index --force`. `stale_files` counts up to 100 paths, while `stale_files_truncated` marks a larger set. `freshness_error` is present when the structural scan could not establish the count; the compatibility value `stale_files: 1` keeps older clients fail-closed. A non-zero value means the next structural query will refresh a managed index, or that a custom external index needs an explicit `mmcg index`. |
| `mmcg_concept` | `query`, optional `top` (default 10, max 50) | Deterministic schema-v2 symbol candidates from normalized names, repository paths, declaration shapes, and owned Rust/Python/JavaScript/TypeScript documentation tokens. Plain terms are escaped and fixed-AND joined; no raw FTS syntax, embeddings, model calls, network, source bodies, raw comments/docstrings, literals, or defaults. The response reports exact indexed-match coverage, bounded-page truncation, and observed safety omissions from one SQLite snapshot. BM25 score is query-local and lower-is-better, not confidence. Managed drift gets at most one refresh/retry; custom indexes remain read-only and fail closed. |

Tool responses are bounded JSON. Collection responses expose their own count or
collection metadata; status and workflow responses use named fields.

Symbol diff matches declarations within their matched parent scopes, retaining
same-name methods in different classes and trait impls. Matching ignores line
shifts and uses signatures to preserve identity when declarations are reordered.
Within one scope/name/kind group, identical signatures match first; repeated
identical signatures are paired in source order. A single remaining pair is a
signature change. Multiple unmatched declarations remain explicit removals/additions
because their pairing is ambiguous. Moving a declaration to a different parent is
also removal/addition.
Changed-test evidence uses the current declaration line and never rematches a
removed test to a surviving same-name declaration. Graph propagation retains its
existing name/type-based precision limits.

Spec snapshot names retain lexical qualification, such as B.run or B::run.
Verification and audit resolve that declaration before comparing its recorded
signature. An unqualified name that matches several declarations produces
snapshot_unresolved, including when one candidate still has the old signature.
File and language constraints from matching frontmatter touches also apply to
snapshot bullets. A missing qualifier never falls back to a leaf-name match.
Malformed names, broken parent chains, failed queries and an index update during
declaration resolution are unresolved, not successful checks. Verification fails;
audit returns Broken. A uniquely
resolved signature change remains Drift in the post-execution audit.

Audit checks recorded signatures and caller counts in both markdown snapshots
and frontmatter touches. Bare touches remain pre-edit existence and file-scope
declarations. A recorded snapshot can be intentionally removed only when
`breaking_changes.removed_symbols` uniquely identifies the same declaration
in the baseline and the diff proves that exact declaration was removed.
Otherwise its absence remains SnapshotSymbolGone/Broken. Removing a class does
not implicitly acknowledge its methods.
Caller counts and risk totals still describe name/type graph candidates in the
declared language, not edges bound to one selected declaration.

Removal acknowledgements accept qualified names and optional file/language scope:

```yaml
breaking_changes:
  removed_symbols:
    - name: B.run
      file: service.py
      language: python
      signature: "def run(self)"
```

A supplied signature is checked after unique identity is established; matching
one overload's signature cannot choose it from several declarations. Bare names
remain supported when unique in the baseline. An unresolved acknowledgement is
`removal_acknowledgement_unresolved` and makes the audit Broken. The public
symbol-diff JSON stays unchanged; internal baseline parser ordinals distinguish
even same-line declarations with identical names and signatures.

Acknowledgements or snapshots without a file scope search all tracked regular
files with supported extractors, including unchanged files. File-scoped checks read
only the required baseline files. Syntax errors, unsupported path encoding,
incomplete diffs and unavailable baseline objects cannot certify uniqueness.
Reads share one deadline and are capped at 10,000 source files, 4 MiB of tree
metadata, 5 MiB per blob, 64 MiB of blobs and 200,000 declarations. Git replacement
refs are ignored and baseline reads do not fetch missing objects. If the global
inventory is incomplete, use precise file scopes or repair the baseline input.
Vue baseline admission also checks its embedded script tree. Multiple script
blocks, external scripts and unsupported script languages remain incomplete
until the extractor can cover them. Quoted `lang="ts"` now selects the actual
TypeScript parser; extractor contract v10 invalidates older indexed output.

`mmcg ci` uses the same proved removal identity when checking snapshots and
touches after execution. It also accepts a deleted touched file when all its
removed declarations are acknowledged. Other missing files, expected docs,
mandatory sections and scoped snapshot checks remain enforced. Standalone
`verify-spec` remains a pre-execution gate and requires those targets to exist.
File scope treats leading `./` and path separators consistently across current
symbols, baseline declarations and deleted-file checks. Invalid relative scopes
cannot grant a deletion exception, including aliases of required docs.

Declared files use one shared admission check in preflight, ordinary audit,
controller postflight and combined CI. Frontmatter `touches`, `creates` and `expected_docs`
remain authoritative when nonempty; otherwise the legacy prose-path fallback
applies only when no structured file scope is present. Invalid or unterminated
frontmatter is a hard declaration error; its readable body is retained for
diagnostics and cannot replace the rejected contract. A declaration must name
a contained regular file using a valid relative path. Directory, symlink/reparse, special-file, absolute, parent and empty paths
cannot satisfy it. Leading `./` and separator aliases are normalized consistently
in scope comparisons, bundles and controller snapshots, independently of cwd.

Admission opens files without following links and rechecks their identity and
metadata through one root capability. It reads no content, imposes no file-size
or UTF-8 requirement, and therefore supports binary assets. It caps one check
at 1,024 declarations and 16,384 work units, with 4,096 path bytes and 64 path
components. Each declaration costs one work unit; each open/recheck costs one
plus its path component count. Checks share a deadline and interruption state.
These receipts establish presence and type, not an atomic snapshot or proof of
file contents.

A missing preflight file retains `missing_file`; other admission failures are
hard `declared_file_unavailable` errors. Postflight admission failures always
produce `declared_file_unavailable` and Broken, including deletion of a required
document whose name appears in the diff. An acknowledged deleted code touch
may be absent only after baseline proof; its absence is rechecked. An
`expected_docs` alias of that touch still requires the file to exist. An unchanged
existing file retains the separate advisory `missing_expected_file` finding.

Rejected declarations stay in diagnostic findings rather than path-typed bundle
fields. Empty, oversized or aggregate targets use `file: null` with an explicit
reason. A failed controller re-audit clears previous approval snapshots and
requires planner review; repair and re-audit retain the initial baseline.
Completed historical tasks keep their existing ordinary-resume behavior; use
`run-task --post-only` to request a fresh audit. External files are not permitted.

Declare additions in a top-level list such as `creates: [src/new.py, docs/new.md]`.
Preflight accepts a regular draft or proven absence after no-follow inspection
of every existing ancestor. Absence receipts recheck the missing component and
ancestor identities. A read error, dangling link or unavailable parent never
grants this allowance. Preflight creates no file or directory. Allowing regular
drafts preserves revised-spec retries and the initial task baseline.

Postflight requires each created file to be regular, present in the added or
untracked diff, and absent from the immutable baseline tree. An untracked old
file removed from the Git index is therefore rejected. Baseline queries read
only names, use literal paths in bounded batches, and share the admission work
budget, deadline and cancellation. A failed query is not proof of absence.

A new required doc belongs in both `creates` and `expected_docs`; an existing
doc remains an ordinary presence obligation. `touches` and `creates` cannot
name the same normalized target. Declared regular files cannot be ancestors
of other declared files. These contradictions fail before filesystem admission.
Pre-edit symbol snapshots, removal acknowledgements and literal FIND retain
their existing requirements; `creates` does not waive them.

Created paths enter audit/report/Bundle scope, history receipts and strict
controller/policy snapshots. Bundle file lists deduplicate normalized aliases;
the input spec hash binds creation versus existing-file roles without changing
the envelope or executor-report schemas. This requires a runtime with `creates`
support: older binaries ignore the field, and updating source does not update
an installed runtime or global workflow package.

Literal `FIND:` blocks are preconditions: `verify-spec` and `run-task --pre-only`
check them against the current working files before execution. A complete read
that lacks the literal produces `find_block_mismatch`. A missing target marker,
invalid path, missing or unreadable file, incomplete read, invalid UTF-8,
changed file, expired deadline or exhausted budget produces the hard error
`find_block_unavailable` with a reason. A failed preflight cannot approve the
task; retry it after repairing the input.

FIND targets must be repository-relative regular files. Leading `./` and path
separator aliases are supported; absolute, parent, symlink and special-file
targets are rejected. Reads use one repository root capability, no-follow opens
and complete UTF-8 contents. Limits per check are 1,024 FIND blocks, 16,384 work
units, 32 MiB of reads, 5 MiB per file, 4,096 path bytes and 64 path components.
Each block costs one work unit; each read costs one plus its path component
count. Repeat reads and final receipt checks share these budgets. Failed reads
consume their reserved byte allowance. Reads also honor the verifier deadline
and the supplied store's interruption state.

Before returning, the checker rereads each inspected file and compares its
identity and content hash, then revalidates the root. A changed or unavailable
receipt replaces even an earlier mismatch with `find_block_unavailable`.
This is not an atomic snapshot of all files or protection against changes after
the check. These read limits apply to FIND checks, not every verifier operation.

Postflight and combined `mmcg ci` do not require the old FIND text to survive a
replacement or acknowledged deletion. The retained Git baseline may differ
from the approved pre-edit working file, so it is not used as a substitute FIND
target. The parser does not model `CHANGE TO` payloads or ordered replacements;
skipping the old precondition does not prove that an edit was applied. Review
the resulting diff and acceptance criteria, and retain the final verification
obligations and ordinary audit gates.

Qualification follows extracted lexical parents. C# namespace segments are
equivalent whether stored as one qualified namespace or nested namespaces.
Synthetic file modules do not represent package names, and package scope is not
inferred from a file path. Rust impl blocks provide method scope but are not
separate named type definitions. Multiple trait implementations, overloads,
constructors and partial declarations can remain ambiguous; snapshots of impl
blocks themselves are not supported by the named-declaration resolver.
An impl removal has one separate structural selector: an explicit file and its
complete exact baseline header, for example
`{name: B, file: service.rs, signature: "impl Marker for B"}`. This selects one
impl block without treating it as another definition of B. Multiple identical
impl headers remain ambiguous; the type and each removed child need their own
acknowledgement. This signature selector does not apply to method overloads.

Rust declaration signatures include their attached outer `#[...]` attributes,
including arguments. Attribute additions, removals and argument edits therefore
appear as `signature_changed` in both symbol diff scopes and change/test impact.
The reported line still points to the declaration. Comments between attributes
and a declaration do not break ownership or enter its signature; inner
`#![...]` attributes are not attached to the following declaration. This covers
the functions/methods, structs, enums, traits, impls and modules emitted by the
extractor, without expanding macros or adding previously unindexed item kinds.

Python signatures also include the full text of attached decorators, in source
order, including multiline arguments. Decorators and the declaration are separated
by newlines so trailing comments cannot hide the next part during normalization.
Adding, removing, reordering or changing
decorators therefore produces `signature_changed` for functions, async functions,
methods and classes. Coordinates still point to `def`/`class`, marker names remain
separate, and decorator-expression calls retain their parent scope. A method
decorator edit can additionally select its containing class as `body_changed`.

When changed files require source extraction, diff and impact reject old
extractor contracts with `index_stale`. Text-only diff needs no extractor index.
Refresh indexes for extractor contract v9. After reindexing, refresh exact-signature
spec snapshots for decorated Python and attributed Rust declarations; bare
`def ...` / `fn ...` snapshots no longer equal the full declaration.
Preserve multiline signatures in YAML rather than legacy one-line
snapshot bullets. Adding a test attribute identifies a changed test candidate;
it does not prove that the test ran or replace the full required gate.

Concept normalization v3 treats Python `//` as division and preserves Python
quote escaping, keeping declaration types searchable while omitting decorator
literals. An older concept corpus also requires refresh, even when the extractor
contract is current.

### Bounded role briefs

`mmcg_brief` and `mastermind brief` call the same builder. CLI JSON is the MCP
logical `structuredContent` packet; text is only a rendering of those fields.
The fixed schema-v1 fields are `repository_content_untrusted`, `role`,
`freshness`, `baseline`, `scope`, `budget`, `changes`, `callers`, `tests`,
`history`, `citations`, `omitted`, `limits`, and `precision_notes` (plus
`schema_version`). Structural and history freshness have separate checked
tokens and statuses.

Candidate caps are 100 changed files, 100 changed symbols, 100 callers, 50
tests, 10 history citations, and eight derived history terms. History performs
at most one quoted OR query and returns only path, kind, query-local rank, and
the source lexemes highlighted by that same FTS5 match; stemming can therefore
make a matched lexeme differ from the derived query term. `omitted` separates
upstream/source limits, rejected unsafe content, and budget admission for every
collection. A null collection `total` and `source_limit_exact: false` preserve
an upstream lower bound instead of inventing an exact count. Planner priority is
changes → callers → history → tests; executor is changes → tests → callers →
history; auditor is tests → callers → changes → history. Stable file-limit and
non-UTF-8-path omissions remain explicit upstream limits in the packet.

The estimate is `ceil(serialized MCP tool-result bytes / 4)`. It includes the
escaped JSON text and structured-content copy but excludes only outer JSON-RPC
framing and request ID. If even the empty fixed packet cannot fit, the tool
returns `{"code":"budget_too_small","minimum_tokens":N}` within the requested
budget. Other stable failures are `invalid_arguments`, `invalid_ref`,
`root_mismatch`, `index_stale`, `schema_incompatible`, `snapshot_changed`,
`work_limit_exceeded`, and `cancelled`. Managed structural or history drift gets
at most one refresh and one retry under the original deadline; custom indexes
remain read-only.

## Watcher (`mmcg watch`)

A long-running process that:
1. Does an initial full index
2. Subscribes to recursive filesystem events under the project root
3. Coalesces rapid-fire events per path with a 500 ms debounce
4. Re-indexes files on Modify/Create, purges on Remove

Architecture: separate process from `mmcg serve`. Both can run concurrently against the same SQLite file thanks to WAL mode (writers don't block readers).

```bash
# Terminal 1: keep the index fresh
mmcg watch

# Terminal 2 (or auto-started by your MCP client): serve queries
mmcg serve
```

## Env vars

| Var | Required | Default | What it does |
|---|---|---|---|
| `MMCG_INDEX_PATH` | no | `.mastermind/mmcg.db` (relative to cwd) | Where the SQLite index lives. |
| `MMCG_QUERY_BUDGET_MS` | no | `10000` for `mmcg serve`; `60000` for `mmcg query`/`mmcg map`/`mmcg impact` | Wall-clock work budget for a single MCP tool call or CLI graph query. `0` = unlimited. See [Work budgets, timeouts, and cancellation](#work-budgets-timeouts-and-cancellation). |
| `MMCG_GIT_TIMEOUT_MS` | no | `30000` | Deadline for bounded Git subprocesses used by history, diff, verification, and audit paths; capped at `300000`. A shorter request deadline still wins. A stuck process is killed and the operation fails with a timeout error. |
| `MMCG_REQUEST_SOFT_TIMEOUT_MS` | no | `30000` | `mmcg serve` only. Wall-clock ceiling after which the watchdog cancels the in-flight request. `0` = no soft ceiling. See [Work budgets, timeouts, and cancellation](#work-budgets-timeouts-and-cancellation). |
| `MMCG_REQUEST_HARD_TIMEOUT_MS` | no | `300000` | `mmcg serve` only. Wall-clock ceiling after which the watchdog exits the process rather than let it keep burning CPU. `0` = no hard ceiling. |
| `MMCG_WATCHDOG` | no | unset | Set to `0` to disable the serve watchdog entirely — both ceilings and the reparent check. |

## Work budgets, timeouts, and cancellation

A single pathological query — a dense name-collision graph, a huge scoped
`mmcg_map`, a stuck `git` subprocess — used to be able to run for hours and
wedge every subsequent call in the session. Two independent mechanisms bound
that now: a **work budget** enforced inside SQLite, and a **watchdog** that
bounds the request as a whole.

The distinction matters. SQLite work is interrupted by its progress handler;
filesystem walks and reads, managed refresh, Git subprocesses, retries, private
snapshot work, and response serialization cooperatively observe the same
absolute request deadline and cancel state. The watchdog remains a final hard
ceiling for code that cannot cooperate.

- **MCP serve** installs `MMCG_QUERY_BUDGET_MS` (default 10,000 ms; `0` =
  unlimited) once around every complete `tools/call`, before the first handler
  attempt. A stale managed index may receive exactly one refresh and one retry,
  but neither resets the deadline.
- **CLI graph queries** (`mmcg query <kind>`, `mmcg map`, and `mmcg impact`) use the same env var with a 60,000 ms
  default — one-shot invocations can afford to wait longer than an
  interactive session.
- Nested internal budgets (e.g. `mmcg_change_impact`'s own tighter 2 s /
  250k-operation cap on its graph walk) compose with the outer budget by
  **minimum** — an inner budget can only tighten the effective deadline,
  never extend it.
- On expiry, MCP tool calls return a structured, typed error instead of
  hanging:
  ```json
  {
    "code": "work_limit_exceeded",
    "budget_ms": 10000,
    "guidance": "narrow scope (subdirectory path, smaller depth, language filter) or raise MMCG_QUERY_BUDGET_MS"
  }
  ```
  `change_impact`'s existing degrade-to-skip behavior for its internal graph
  budget is unchanged — that specific interrupt is caught and reported as a
  `graph_work_limit` precision note, not a hard failure of the whole call.
- **Cancellation.** An MCP cancel notification (`notifications/cancelled`,
  and legacy `$/cancelRequest`) for the in-flight request id interrupts the
  running query and frees the server for the next request. A cancel is
  reported as `{"code": "cancelled", ...}` — distinct from
  `work_limit_exceeded`, and a cancel that arrives after its request already
  completed never aborts the next one. Known limitation: the serve loop is
  still serial, so an unrelated request (even `ping`) sent while a query is
  running still waits — bounded by the work budget, not eliminated by
  cancellation.
- **Git subprocesses** used by history, diff, verification, and audit paths are
  killed if they exceed `MMCG_GIT_TIMEOUT_MS` (default 30,000 ms, maximum
  300,000 ms). A shorter request deadline still wins. The per-file diff loop is
  additionally capped at 10,000 files; beyond that, `truncated: true` marks
  the response as a prefix, not the full diff.
- **Git refs** are non-empty and at most 1,024 bytes, cannot begin with `-` or
  contain NUL, and must resolve through `rev-parse --verify --end-of-options`
  to one full lowercase SHA-1 or SHA-256 commit OID (40 or 64 hex characters).
  Later Git commands use that OID and an explicit `--` path separator, never
  the raw caller-supplied ref.
- **Serve watchdog.** `mmcg serve` runs a polling thread that measures each
  in-flight request on the wall clock, independent of where the time is being
  spent. It escalates: at `MMCG_REQUEST_SOFT_TIMEOUT_MS` (default 30,000 ms) it
  cancels the request exactly as a client cancel would; at
  `MMCG_REQUEST_HARD_TIMEOUT_MS` (default 300,000 ms) it exits the process. The
  hard ceiling exists because a cancel only lands on SQLite work — if a request
  is wedged somewhere else, exiting is the only bound left, and MCP clients
  respawn stdio servers cleanly. Set `MMCG_WATCHDOG=0` to disable both.
- **Reparent check.** The same thread exits when the server's parent process
  changes. `bin/mmcg.js` spawns the binary with `stdio: "inherit"`, so stdin
  belongs to the MCP client rather than to the node wrapper — if the wrapper
  dies, EOF never arrives on its own and the server would otherwise linger
  indefinitely.
- **Connection defaults.** Every index open sets `busy_timeout = 5000` and
  `cache_size = -65536` (64 MiB), sane defaults for multi-hundred-MB
  databases under concurrent `serve`/`watch` access.
- **`mmcg_dependency_cycles`** additionally caps the import graph it feeds to
  Tarjan's algorithm at 50,000 distinct file-pair edges. Above the cap, the
  response reports `truncated: true`, `truncation_reason: graph_work_limit`,
  null totals, and an empty `cycles` list — SCC
  analysis is skipped entirely rather than run on a partial graph, because a
  capped cycle detector can split or hide real cycles. This is "incomplete
  and possibly inaccurate", never just "more available"; narrow with
  `language` and retry. MCP output separately caps complete SCCs by `top` and
  by 500 total file memberships. It never labels a partial membership list as
  a cycle; use the complete CLI query when `member_limit` truncates the result.

## Limitations

### Language and extraction coverage

- Supported extensions are listed in [Language coverage](#language-coverage).
  Matching is case-insensitive. Other extensions are skipped and reported in a
  bounded diagnostics sample.
- A Vue SFC becomes one component named after the file. Its script uses the
  TypeScript or JavaScript grammar and retains `.vue`-absolute lines. Template
  tags emit calls, but template expressions and build-tool auto-imports do not.
  Kebab-case and PascalCase tags normalize to PascalCase.
- Python module constants are direct module-child assignments only. Assignments
  inside control flow, classes, or functions are not constants. Constants are
  excluded from `mmcg_unreferenced` by default because Python value reads are
  not general reference edges; request `kind=constant` explicitly.
- Rust records known function values and parsed macro-body usages as
  `references`. These usages do not establish invocation or expand macros.
  Macro-body reparsing is bounded to 64 KiB per body, 1 MiB of reparsed input
  and 256 parse attempts per file, and eight nesting levels. Unsupported or
  over-budget bodies and wildcard-imported function values can be omitted.
  Query `truncated` fields do not report these extraction omissions.
- C/C++ parsing has no preprocessor, template instantiation, ADL, or overload
  resolution. Macros such as `TEST(Suite, Name)` are calls, not definitions.
  Header declarations and source definitions remain separate rows. Include
  resolution tries exact repository-relative, source-relative, then
  deterministic suffix matches and may over-approximate duplicate basenames.
  One `tree-sitter-cpp` grammar handles C and C++; unusual C identifiers that
  are C++ keywords can mis-parse. SCIP can add resolved evidence separately.

### Symbol and edge identity

- Call targets are name-based candidates. Where available, syntax constrains
  the target kind so a receiver call does not bind a bare free function, but it
  does not establish receiver type or compiler identity. `obj.foo()` retains
  `to_name=foo` and literal `to_path=obj.foo`.
- Default edges store `to_name` and `to_path`, not a resolved cross-file
  `to_id`. A callers query can therefore combine unrelated same-name symbols.
- Import `match: path` compares literal source spelling. A re-exported
  `crate::Baz` does not match `foo::bar::Baz`. Use `match: name` for broad
  syntactic consumers or a SCIP overlay for compiler identity.
- TS, JS, and Python member calls use the rightmost capitalized receiver as
  `to_type`. Rust scoped identifiers do not need this heuristic. Uppercase
  variable names can produce false type hints.
- JSX calls follow the uppercase component convention. `<Button />` is a call;
  `<div>` is not. Lowercase custom components are missed and capitalized host
  elements in custom renderers can be counted.
- Variable-declared functions use their binding name. One wrapper layer such as
  `memo(...)`, `forwardRef(...)`, or `styled(...)` is unwrapped. Nested wrappers
  can be missed; a non-function assignment that receives a callback can be
  over-captured.
- C# partial declarations collapse only when name, kind, and full namespace
  identity match. Legacy rows without namespace identity stay separate.
  `mmcg_callees` selects a declaration by `file` and `line` when needed;
  outgoing target names and transitive dependencies remain candidates.

Graph precision is at most `medium` for Rust, Go, Java, C#, and other supported
AST extractors, and `low` for C/C++. `resolution` describes extraction strategy
(`syntactic` or `heuristic`), while `target_resolution: name_based_candidates`
states the absence of compiler/type resolution. SCIP evidence has separate
provenance and does not silently upgrade these default graph queries.

### Heuristic query semantics

- Impact follows both call and reference candidates, so dependency reach is
  not runtime call reach. Empty results do not establish absence of
  dependencies. Precision notes preserve this distinction even when no query
  limit was reached. Graph-selected tests have at most `medium` confidence;
  `high` is reserved for a test symbol directly observed to change.
- `disciplines` derives only path-level signals. Frontend extensions, test
  naming, and migration paths can trigger a discipline; everything else is
  `unclassified`. A migration signal indicates review depth, not destructive
  behavior.
- `mmcg_api_surface` is empirical: it returns symbols currently referenced from
  outside a prefix, regardless of declared visibility. It is not a language
  public-API declaration query.
- `mmcg_recent_changes` reports the source mtimes stored in the index, not the
  time indexing ran or Git history. A stale index can omit newer worktree
  changes. After a rebase or forced re-index, use `git log --since=...` for Git
  truth.
- `mmcg_unreferenced` suppresses recognized framework entry points:
  pytest fixtures/marks, common web routes, JIT/task/CLI decorators, Rust test
  attributes, C# test/web/benchmark attributes, JUnit/Spring annotations, and
  PHPUnit/Symfony/Livewire attributes. It also filters `test_*` functions in
  test/spec paths. It cannot reliably see undecorated entry points, dynamic
  dispatch, cross-language calls, runtime registries, C++ macro tests, or Go's
  `TestXxx` convention. Treat every result as a candidate to inspect, never as
  authorization to delete code.

### Operational behavior

- A watcher observes files created inside a new deep directory individually;
  it does not rescan the entire new subtree as one special event.
- A schema-version change rebuilds derived graph and task-search tables.
  Repository identity, project-history search, and scratchpad entries are
  retained so `mmcg index .` can repopulate the graph in place.

## CI

`mastermind ci` indexes the repository, verifies selected specs, parses their
executor reports, audits the real diff, and optionally emits sealed bundles.
Bundle construction reads the bound spec and executor report through one
retained repository capability and rejects path substitution, symlinks, and
special files before recording their digests.
For pull requests, scope the gate to changed task folders and require evidence:

```bash
mastermind ci --since origin/main \
  --changed-only --require-executor-report \
  --bundle-dir .mastermind/audit-output
```

Without `--changed-only`, the command retains its compatibility behavior and
walks all task specs. CI bundle publication always requires a canonical
`executor-report.md`, even if the explicit requirement flag is omitted.
Changed-task discovery resolves the requested baseline and HEAD to commit IDs,
uses a literal task-directory pathspec, and shares the configured bounded Git
deadline while ignoring ambient repository-routing and external-diff settings.
Task discovery has a fixed entry limit, does not follow linked directories or
contracts, and maps changes anywhere below a task folder back to its spec.

Canonical schema-v1 reports retain their task path, completion status, phases,
modified-file declarations, defects and verification excerpts. A `partial` or
`failed` report produces `executor_report_rejected` and a `broken` audit, even
when there are no claims. Its `spec` must identify the audited task inside the
repository; absolute and repository-relative paths are supported. Reported
files and phases do not replace the spec's scope or prove plan coverage.

`run-task` postflight and CI with `--require-executor-report` or `--bundle-dir`
reject legacy reports. Ordinary `audit-spec` and CI without those flags retain
legacy compatibility. Canonical parsing rejects explicit nulls, invalid scalar
types and ambiguous sentinel blocks. Repository-owned specs and reports are
read through bounded, no-follow snapshots. Explicitly selected report files
also reject a path that changes during the read.
Markdown sentinel comments occupy their own unindented lines; marker text
inside YAML string evidence remains data.
Empty phase/verification lists remain valid under v1. Canonical postflight also
requires a passing reported result for every nonempty `verify[].cmd` and each
recognized `VERIFY:`, `**VERIFY**:` or `**VERIFY:**` command line in the spec.
Labels and ordinary shell fences do not declare machine-checked obligations.
`verify-spec --strict` requires at least one such command; labels or blank
`cmd` values cannot satisfy that requirement.

Command matching trims outside whitespace only. Arguments, case, wrappers and
internal whitespace remain significant; separate rows cannot satisfy a compound
command. Missing commands produce `verification_requirement_unmet` with
`missing_result`. A matching row that does not claim a pass, reports a nonzero
exit, or reports zero tests for a recognized test run produces `not_passed`;
mixing it with passing rows produces
`conflicting_results`. These findings make the audit and bundle `broken` and
return the controller to planner review. Repeated successful rows may have
different excerpts or positive test counts. Optional observations remain
optional. Empty results can pass coverage
when the spec has no command obligations.

For a claimed pass, a nonzero `observed.exit_code` produces
`observed_exit_code_non_zero` for any command. `observed.tests_run: 0` produces
`observed_zero_tests` only for recognized test execution, whether the exit code
is zero or omitted. A positive count cannot override a nonzero exit. Missing
observations are not converted to successful execution or to a zero count.

Recognition covers a small subset of direct `cargo test`, `go test`, `pytest`,
`python -m pytest`, `python3 -m pytest`, `jest`, and explicit `vitest run` or
`vitest --run` invocations, including `.exe` runner names. Plain paths, filters
and selected runner-specific options are supported; use `vitest run` for bare
positional filters because `--run` can still select another subcommand.
Option values retain their
runner-specific meaning. Compile, collection, help/version and watch modes do
not establish a finite test run. Unsupported flags, forwarded harness arguments,
shell quoting/expansion, wrappers and arbitrary npm/yarn scripts remain unknown.
Zero counts for these commands do not by themselves make an audit broken.
Command recognition does not inspect implicit configuration or authenticate a
run. It does not change the exact command matching used for coverage.

`vacuous_test_claim` remains an advisory filesystem warning for recognized test
commands when no positive count is reported. Finding no conventional test files
does not prove zero execution, and finding files does not override an explicit
zero count. Non-test and unknown command forms skip this heuristic.

Only a completed scan with rechecked directory entries, path kinds and Rust
source receipts can emit this warning. External paths, parent traversal,
symlinks, special files, read errors, invalid UTF-8 and incomplete rechecks make
the advisory result unknown and suppress the absence warning. Unknown is not
evidence that tests exist or ran. Hard reported-outcome contradictions are
checked independently, including after the scanner exhausts its budget.

One repository capability and one budget cover the entire report. The scanner
allows 16,384 work units for enumeration and inspection, 8 MiB of total source
reads and 1 MiB per file. Rechecks consume the same limits. Descendant traversal
is capped at depth 12 and explicit starting scopes at 64 path components. The
existing audit deadline and Store cancellation/budget marker also apply.
Paths recorded in Git metadata, indexed declarations and canonical task bindings
resolve against the selected repository root even when the audit starts from a
nested working directory.

Scopes stay conservative: Cargo checks conventional `src/` and `tests/` under
one crate directory, falling back to that directory when both are absent.
Package, workspace and doctest selectors remain unknown. Go supports one local
directory and terminal `/...` recursion; pytest supports one directory or
explicit Python file, including both `test_*.py` and `*_test.py` discovery names.
Multiple targets, pytest node selectors and positional JavaScript filters remain
unknown. Git metadata is excluded. These are conventional-file scans, without
evaluating package configuration, plugins or the runner's complete discovery
rules; the rechecks do not create an atomic filesystem snapshot.

The executor runs commands and records their results. Coverage checks compare
self-reported evidence with the spec; they neither run commands nor authenticate
execution. Ordinary legacy reports retain their earlier compatibility behavior.

The JSON audit result includes the complete checked `executor_report` when one
was supplied. Bundle creation binds all its metadata and verification evidence
as well as claims; replacing the report with one containing identical claims
but different completion/task data produces a broken bundle. When the caller
omits the report during Bundle construction, the checked snapshot is reused.
Sealing requires its input file and rechecks that file against the snapshot.

### Executor claim evidence

`audit-spec --executor-report ... --json` includes ordered `claim_checks` with
the complete original claim, its zero-based index, `verified`, `failed`, or
`unresolved` status, and either evidence or a finding. An omitted field means
claims were not evaluated; `[]` means an empty report was evaluated. Canonical
executor input remains schema v1.

`function_added` must select one exact lexical declaration before comparing a
signature. It must also correspond to a current declaration introduced in that
file by the baseline diff. A body edit, signature edit, or matching overload is
insufficient. Indistinguishable parent declarations and edited overload sets
produce `executor_claim_unresolved`; existing declarations produce
`claimed_symbol_not_added`. Addition evidence records the file, line and exact
baseline OID. This is not move detection across files.

`integration` supports `relation: calls` (also the default). Both endpoints must
resolve uniquely. Evidence preserves call target kind/type and requires one
compatible indexed candidate across files in the source language; `to_file`
cannot hide another compatible target. The result is explicitly
`compatible_call_candidate` with `name_based_candidates` precision. It does not
establish compiler binding, receiver type, alias resolution or a runtime call.
Callbacks/references, unsupported relations, unresolved prefixes and ambiguous
candidates cannot satisfy it.

Changed source files and claim endpoint files must match the index content
hashes and parse without recovery. Query failures, stale files, incomplete
diffs, index changes, and exceeded claim/query budgets remain unresolved and
make the audit `broken`. The same outcomes drive CI and controller postflight.

Schema-v3 bundles retain their field layout and add the two finding kinds above
to the publisher's strict whitelist. Verified/failed claim labels carry ordinal
positions, so repeated names with different files or signatures remain separate.
Bundle creation requires outcomes for the exact complete claim sequence;
unchecked or substituted reports produce a broken bundle with no verified
claims. `mmcg_queries` provides inspection entry points for verified claims,
not an execution trace.

### Schema-v3 audit envelopes

`mastermind audit-spec ... --bundle evidence.json` seals the mechanical report as canonical JSON with a manifest SHA-256 digest. A valid digest is tamper-evidence, not provenance: verification succeeds only with a complete exact repository/baseline/head/root/clean-worktree policy, a required Ed25519 signature rooted in an allowlisted non-revoked key ID, or both. `mastermind audit verify ... --integrity-only` is labelled untrusted and reports authenticity and policy as `not_evaluated`.

Detached signatures use a domain-separated schema-v1 statement binding the envelope schema, hash algorithm, canonicalization, key ID, and manifest digest. Private keys are single-line base64 32-byte Ed25519 seeds with Unix mode 0600. Key owners are responsible for rotation, revocation allowlists, and protecting historical verification policy; Ed25519 alone provides neither signing time nor pre/post-compromise distinction.

The Docker Action requires exact full baseline and head OIDs, a GitHub `owner/repo`, and a clean worktree. Its publication example treats `workflow_run` artifacts as hostile until independent run, attempt, workflow blob, PR, artifact ID/digest/size, envelope, and policy checks pass. The resulting attestation means only publication-workflow verification provenance; it does not prove that PR analysis ran in a trusted environment or that the findings are true.

`.github/workflows/ci-mmcg.yml` runs the full test suite plus an end-to-end smoke (`mmcg doctor --json` + `mmcg verify-spec` + `mmcg audit-spec` against `tests/ci-fixture/`) on a 6-target matrix every PR: x86_64/aarch64 Linux gnu + musl, aarch64 macOS (Apple Silicon), x86_64 Windows. macOS Intel (`x86_64-apple-darwin`) is not gated per-PR but builds locally via `cargo install --target=x86_64-apple-darwin`.
