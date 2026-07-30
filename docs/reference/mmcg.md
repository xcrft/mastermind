# mmcg — Mastermind Codegraph

This is the exhaustive technical reference for the mmcg engine. For the shortest path to a working installation, start with [Getting started](../getting-started.md); for the product overview, return to the [Mastermind README](../../README.md).

A small Rust binary that builds a structural index of a codebase (Python, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust, C#, Go, Java, PHP, C/C++). It serves the graph over **MCP** (23 read-only tools plus one additive local scratchpad write), but MCP is only one surface. The same binary provides spec gates (`verify-spec` / `audit-spec`), project setup (`init` / `doctor`), and the user-global style miner.

> Installed via npm (`@xcraftmind/mastermind`)? The command is **`mastermind`** — the same binary. The `mmcg` name used throughout this doc is the cargo-installed alias (`cargo install mmcg`).

## What it indexes

For each supported file, mmcg captures:

| Construct | Python | TS/JS | Rust | C# | Go | Java | PHP | C/C++ |
|---|---|---|---|---|---|---|---|---|
| Functions | ✓ `def` | ✓ `function` | ✓ `fn` | ✓ method/local-fn | ✓ `func` | ✓ `method_declaration` | ✓ `function` | ✓ `function_definition` |
| Module constants | ✓ `FOO = ...` (direct module children only) | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| Methods | ✓ inside classes | ✓ inside classes | ✓ inside `impl`/`trait` | ✓ inside classes/etc | ✓ `func (r T) M()` | ✓ inside classes | ✓ inside classes | ⚠️ inside classes + `T::m()` def |
| Types | ✓ `class` | ✓ `class`, `interface` | ✓ `struct`, `enum`, `trait` | ✓ `class`, `struct`, `interface`, `record`, `enum` | ✓ `struct`, `interface` | ✓ `class`, `interface`, `enum`, `record` | ✓ `class`, `interface`, `trait`, `enum` | ✓ `class`, `struct`, `union`, `enum` |
| Calls | ✓ | ✓ + `<Component />` JSX | ✓ + `Mod::foo()` | ✓ + `Type.Method()` | ✓ + `pkg.Func()` + `Foo{}` | ✓ + `new Foo()` | ✓ + `Foo::bar()` + `new Foo` | ⚠️ + `new Foo` (no template inst) |
| Decorators/attributes | ✓ `@pytest.fixture` | n/a | ✓ `#[test]` | ✓ `[Fact]` | n/a | ✓ `@Test`, `@GetMapping` | ✓ `#[Test]` (PHP 8) | n/a |
| Imports | ✓ `import`/`from` | ✓ ES module forms | ✓ `use` paths | ✓ `using` | ✓ `import` | ✓ `import` | ✓ `use` | ⚠️ `#include` (text-only) |
| Macros | n/a | n/a | ✓ `println!` | n/a | n/a | n/a | n/a | ❌ invisible — see Limitations |
| **FQ path** | `collections.abc.Iterable` | `'pkg'::default` | `foo::bar::Baz` | `System.Collections::*` | `net/http::*` | `java.util.List` | `App\Foo` | `vector::*` |

Each file gets a synthetic `<module>` symbol (kind `module`) that owns module-scope imports and top-level statements.

### Language coverage

Honest per-language summary — what the indexer captures and where it stops:

| language | symbols | calls | imports | known gaps |
|---|---|---|---|---|
| Python | function, method, class, constant | ✓ direct + `obj.method()` (capital-letter receiver heuristic) | ✓ `import`/`from … import` with aliases and `as` rebinds | star-import expansion not tracked; dynamic `getattr` dispatch invisible; `__all__`-filtered re-exports not linked |
| TypeScript | function, arrow-fn, method, class, interface, type-alias | ✓ + `new Foo()` constructors + method calls + `<Component />` JSX usage | ✓ ES named / default / namespace + re-exports | anonymous default exports lose name; `export * from` re-exports not expanded to member level; JSX component detection is the uppercase-tag convention |
| JavaScript | function, arrow-fn, method, class | ✓ same walker as TS (TS-only node kinds skip silently), including JSX in `.jsx`/`.js` | ✓ ES named / default / namespace | no CommonJS `require()` as import edge; same gaps as TS minus interface / type-alias |
| Vue SFC | component (the file), plus every script symbol via the TS/JS walker | ✓ `<BaseButton />` and `<base-button />` template usage, normalized to PascalCase | ✓ the script block's ES imports | template expressions (`@click="bump"`, `:prop="expr"`) are attribute text, not parsed; auto-imported components (`unplugin-vue-components`) have no import edge; `<script>` and `<script setup>` merge into one symbol set |
| Rust | function, method, struct, enum, trait, impl-block, mod, macro-call | ✓ + `Crate::fn()` scoped calls + `macro!` invocations | ✓ `use` paths with aliases and globs | proc-macros invisible at parse time; `derive` traits stored as decorator not call edge; glob `use foo::*` recorded as `*` (no member expansion) |
| C# | class, struct, record, interface, enum, method, property, namespace | ✓ + `new Foo()` constructors + method calls | ✓ `using` directives + type aliases | anonymous lambdas unnamed; LINQ extension calls not tracked individually; `partial class` stored per-file (collapsed on query by default) |
| Go | function, method, struct, interface, type | ✓ + composite literals `Foo{}` + pkg-qualified `pkg.Fn()` | ✓ `import` paths with aliases, blank identifier, dot imports | goroutine launches not marked semantically; anonymous closures unnamed; build tags stored as decorator |
| Java | class, interface, enum, record, method, constructor | ✓ + `new Foo()` + method-call expressions | ✓ `import` declarations including static + wildcards | anonymous inner classes not tracked; lambda bodies unnamed; annotation processors invisible |
| PHP | namespace, class, interface, trait, enum, method, function | ✓ + `new Foo`, `Foo::bar()`, `$this->method()` | ✓ `use` with aliases + grouped `use App\{A, B as C}` form | magic methods (`__get`, `__call`) tracked as symbols but call targets unresolved; `call_user_func` target invisible |
| C/C++ | function, method, class, struct, union, enum | ⚠️ best-effort — no preprocessor, no semantic analysis | ⚠️ `#include` as text-only import edge | macros invisible (`TEST(Suite, Name)` parsed as a call not a def); template instantiations not tracked; header/source split produces duplicate rows (no dedup); ADL/overload not resolved |

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
These directories are always skipped: `.git`, `.mastermind`, `.venv`, `venv`,
`__pycache__`, `node_modules`, `target`, `dist`, `build`, `.tox`, `.pytest_cache`,
`.mypy_cache`, `.ruff_cache`, `.next`, `.turbo`, `.cache`.

## Why a custom indexer

The Mastermind workflow needs structural queries every few seconds (planner deciding blast radius, executor checking callers before edits). Grep/Read each costs hundreds of tokens; mmcg returns the same info in dozens. Multiplied across a workflow run, the difference is between affordable and not.

mmcg is intentionally narrow:
- Nine languages (Python, TypeScript/TSX, JavaScript/JSX, Rust, C#, Go, Java, PHP, C/C++) — extend by adding a parser, not by depending on multi-language toolchains
- 24 query tools that map directly to the workflow's needs
- 23 read-only MCP tools plus `mmcg_scratchpad_append`, which makes an additive write to the gitignored local index

## Performance model

- **Parsers**: tree-sitter (C, vendored — no system tree-sitter required)
- **Parallelism**: `rayon` parses files in parallel; writes serialize through a single SQLite connection (WAL mode)
- **Bounded batching**: at most 64 parsed files are retained before the single SQLite writer commits them; each file uses one transaction via `Store::commit_file`
- **Source admission**: source-looking files above 5 MiB or containing a NUL byte in the first 8 KiB are skipped before parsing
- **Storage**: SQLite with indexes on `symbols.name`, `edges.from_id`, and `edges.to_name`

Index time depends on repository size, filesystem, hardware, and whether the run
is incremental. Use `mmcg index . --force` for a cold parse measurement and
record the emitted file/symbol/edge counts with the timing; do not compare a
cold run with an incremental no-op. Maintainers can run `just benchmark-index`
for a reproducible synthetic cold/warm/incremental report including peak process
RSS. It is an evidence tool, not a machine-independent CI threshold.

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

# Watch a directory and re-index on file changes (long-running, also incremental)
mmcg watch

# Show what's in the index
mmcg status

# Search durable decisions, reports, audits, and lessons. `why` renders an
# evidence envelope and never invents rationale absent from the records.
mmcg history "webhook dedupe"
mmcg history "runtime boundary" --kind audit
mmcg why "why is webhook dedupe durable?"

# Build one bounded schema-v1 project map with safe text, JSON, or Mermaid projections
mmcg map . --format text
mmcg map src --format json --depth 2 --top 20
mmcg map . --format mermaid

# Analyze baseline vs staged, unstaged, and untracked changes.
mmcg impact --since main --format text --depth 3 --top 100
mmcg impact --since HEAD~1 --format json

# Health-check the project setup (index, gitignore, CLAUDE.md, MCP config,
# `mmcg serve` handshake). Exit code 1 if any check fails — wire into CI.
mmcg doctor                                          # human-readable report
mmcg doctor --json                                   # machine-parseable

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
mmcg run-task .mastermind/tasks/042-feature/spec.md --reset     # drop state, force pre-flight (counter survives)
mmcg run-task .mastermind/tasks/042-feature/spec.md --pre-only  # never auto-resume into post
mmcg run-task .mastermind/tasks/042-feature/spec.md --post-only # requires state
mmcg run-task .mastermind/tasks/042-feature/spec.md --allow-no-index  # docs-only / spec-only specs
mmcg run-task .mastermind/tasks/042-feature/spec.md --strict          # fold strict spec checks into pre-flight
mmcg run-task .mastermind/tasks/042-feature/spec.md --max-iterations 5 # raise the default budget (default 3)
mmcg run-task .mastermind/tasks/042-feature/spec.md --force-iteration  # bypass budget; deduplicated lesson candidate records the signal
# NOTE: without --allow-no-index, pre-flight hard-fails when the index is missing
# or empty. Gates without a codegraph degrade to file-existence + section checks
# only — mmcg's value comes from the structural truth layer, not the heuristics.

# Initialize a project. Stack detection informs drafting, while CONTEXT stays
# lean and stack-agnostic; commands and layouts belong in CLAUDE.md.
mmcg init
mmcg init --no-claude      # skip Claude-assisted context drafting
mmcg init --no-index       # scaffold without building the graph
mmcg init --no-global      # do not reconcile the npm Claude workflow bundle
mmcg init --no-seed-style  # do not enrich ~/.mastermind/style.md

# Build or refresh the user-global personal style profile. No init required.
mmcg miner profile .
mmcg miner profile . --author "Ada Lovelace"  # explicit git author filter
mmcg miner profile . --deep                    # explicit claude -p compatibility path
# --force intentionally replaces the whole profile, including preserved prose;
# it is not refresh.

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
mmcg query callees parse_one
mmcg query impact extract --depth 3
mmcg query files --prefix src/indexer
mmcg query outline src/store.rs                    # symbol tree of one file
mmcg query recent --since 2h                       # files re-indexed in last 2 hours
mmcg query unreferenced --kind function            # dead-code candidates (review manually)
mmcg query api-surface src/runtime/                # symbols under prefix used externally
```

### Incremental indexing — how it works

When you run `mmcg index`, mmcg compares each file's filesystem mtime against the mtime stored in the index:

- **ignored by Git or `.ignore` rules** → do not scan; a newly ignored indexed file is purged
- **mtime newer than stored** → re-parse and commit (counted as `indexed`)
- **mtime equals stored** → skip without parsing (counted as `unchanged`)
- **file in index but not on disk** → purge from index (counted as `purged`)
- **binary-looking or larger than 5 MiB** → skip safely and report separately
- **unsupported extension** → skip (counted as `skipped`)

The database also stores an extractor-contract version. When parser/extractor
semantics change, the next ordinary `index` run automatically performs a full
structural rebuild even when file mtimes are unchanged. `status`, `doctor`, and
`mmcg_status` expose contract drift so agents do not trust structurally stale data.

Output example:

```
indexed 3 (unchanged 124, purged 1, skipped binary 2, skipped large 1, failed 0) / scanned 1247 | 87 symbols | 412 edges | 4 task specs | 84 ms
```

When to use `--force`:
- After a schema version change (schema and extractor-contract mismatches already rebuild automatically, but `--force` lets you request a cold rebuild explicitly)
- If you suspect the index is stale for reasons mtime can't see (e.g., a file was restored from backup with old mtime)
- For benchmarking — to see how long a cold index takes

**Orphan purge caveat.** mmcg purges any indexed path that was not seen during this run's walk. This is the right behavior when you run `mmcg index <same-root>` every time. But if you switch roots between runs (e.g. `mmcg index .` then `mmcg index src/`), the second run will see different paths and wrongly purge the rest.

**Best practice:** pick one project root (usually `.` from your project's top directory) and stick with it. `mmcg watch` always uses the root you pass at startup. If you accidentally indexed a different root, run `mmcg index --force <correct-root>` to rebuild from scratch.

The index lives at `.mastermind/mmcg.db` in the current directory by default. Override with `--index <path>` or env var `MMCG_INDEX_PATH`.

## MCP server usage

```bash
mmcg serve
```

### Protocol contract

- Supported revisions are MCP `2025-11-25` and legacy `2024-11-05`. Unknown requested revisions receive the latest supported revision; clients that cannot support it disconnect.
- Current results include compact JSON text and object `structuredContent`. Scratchpad reads keep their JSON-array text form and are wrapped as `{ "entries": [...] }` only in structured form. Legacy results remain content-only.
- Tool execution and input errors use `isError: true`; malformed protocol requests and internal failures use JSON-RPC errors.
- Input frames are limited to 1 MiB and an oversized frame closes the connection. Serialized result payloads are limited to 8 MiB and ask the caller to narrow the query.
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

## MCP tools

| Tool | Args | What it returns |
|---|---|---|
| `mmcg_search` | `name`, optional `kind`, `language`, `collapse_partials` (default `true`) | Symbols matching exactly. Pre-flight "does X exist?" check. Returns location, kind, signature, and any decorators/attributes. C# `partial class` declarations across N files collapse into one hit with a `locations` array of all N declarations; pass `collapse_partials: false` to see every declaration separately. |
| `mmcg_callers` | `name`, optional `language`, `edge_kind` (default `calls`) | **Containing functions** that reference `name` by the given edge kind. Count = distinct containing units, not distinct call sites (a function with 3 calls to `name` counts once). Use before editing for blast radius. Pass `edge_kind: imports` to find importers via the same tool. |
| `mmcg_callees` | `name`, optional `language`, `edge_kind` (default `calls`) | Names the symbol references via the given edge kind. |
| `mmcg_impact` | `name`, optional `max_depth` (1-10, default 2), `language` | Transitive callers via `calls` edges. Full blast radius. Bounded at 5,001 rows: above that, `truncated: true` is returned alongside `row_limit` and the (partial) `impact` list — narrow `max_depth` or add a `language` filter to see the rest. |
| `mmcg_imports` | `file` | Names imported by this file's top-level imports — each entry has `name`, `path` (fully-qualified), `line`. |
| `mmcg_imported_by` | `query`, optional `match: name`(default)/`path`, `language` | Files whose imports reference the given name OR fully-qualified path. Use `match: path` when the leaf name is ambiguous across modules. |
| `mmcg_symbols_in_file` | `file` | All symbols defined in a file, source order. Flat list. |
| `mmcg_outline` | `file` | Symbol tree of a file — classes/impls own their methods, modules own top-level functions. One call replaces a search + multiple lookups. |
| `mmcg_files` | optional `prefix`, `language` | Indexed files with symbol counts. |
| `mmcg_recent_changes` | `since` (e.g. `2h`, `30m`, `1d`) | Files re-indexed within the given window. Useful for "what changed recently?" during incident investigation. |
| `mmcg_scratchpad_append` | `agent`, `kind`, `body` | Append a one-line intent / note / handoff to the cross-agent scratchpad — live in-session channel between Mastermind subagents (planner → executor → auditor). Persists in `.mastermind/mmcg.db`. Body capped at 8 KiB. Cross-session counterpart is `_lessons.md`. |
| `mmcg_scratchpad_read` | optional `since`, `agent`, `kind`, `limit` | Read recent scratchpad entries, newest first. `since` is a unix timestamp (seconds); omit for the last `limit` entries (default 20, max 200). |
| `mmcg_change_class` | `file` | Classify a file's last change as `structural`, `cosmetic`, or `first-seen`. Backed by an FNV-1a 64-bit hash of the file's parsed structural shape — line numbers and whitespace excluded. Pre-edit signal for planner and auditor: large diffs that are mostly cosmetic have smaller real scope than line count suggests. |
| `mmcg_unreferenced` | optional `kind`, `language` | Symbols that no edge references. Dead-code candidates. **Review manually** — see Limitations for false-positive scenarios. |
| `mmcg_api_surface` | `prefix`, optional `language` | Symbols under `prefix` referenced from at least one file OUTSIDE `prefix`. Empirical "who-uses-this-module" map; doesn't need declared visibility. |
| `mmcg_centrality` | optional `prefix`, `language`, `kind`, `top` (default 20) | Rank symbols by in-degree (distinct callers). Pre-flight "where is the gravity" — top hits are the structural attractors of the codebase or a subdirectory. Use to learn what to read first on unfamiliar code. Excludes synthetic `<module>` rows and zero-degree symbols. |
| `mmcg_map` | optional `path` (default `.`), `depth` (1–6, default 2), `top` (1–100, default 20), `production_only` (default `false`) | Schema-v1 architecture briefing with lexical file/directory scope: `%` and `_` are literal bytes, selected-directory components are relative to that directory, root components remain repository-relative, and selected files retain their paths. `production_only` excludes conventional test, fixture, example, demo, benchmark, generated, vendor, dependency, and build-target path segments before bounded queries run. Caps are 50,000 aggregation paths, 20 languages, 20 components, 20 boundaries/component and 400 globally, 50 entry points, 100 hotspots, 50,000 scoped cycle edges, 50 cycles, and 500 cycle memberships. `path_work_limit` marks path-derived partial aggregates; `top_probe` marks a hotspot or per-component boundary cap+1 probe; `global_probe_limit` marks components whose certainty was prevented by the 401st global boundary row; cycle `work_limit` returns no cycles because SCC analysis was skipped before truncated edges could be analyzed. |
| `mmcg_change_impact` | `since`, optional `root`, `depth` (1–5), `top` (1–500) | Stable schema-v1 analysis of the resolved baseline against staged, unstaged, and untracked content. Reports added/removed/signature/body-changed symbols, batched transitive callers, component crossings, ranked test candidates, a `disciplines` block routing the change to an evidence set, exact collection metadata, caps, and precision notes. Root, SHA-256 index freshness, Git snapshot, and SQLite snapshot checks fail closed with stable codes. |
| `mmcg_test_impact` | `since`, optional `root`, `depth` (1–5), `top` (1–500) | Exact test-focused projection of `mmcg_change_impact`. Changed tests and depth-1 graph tests are direct, deeper graph tests are transitive, and scoped filename candidates are heuristic. Focused candidates never replace the repository's full required gate. |
| `mmcg_tasks` | `query`, optional `top` (default 10) | Full-text search past task specs (`.mastermind/tasks/<NNN>-<name>/spec.md`). FTS5 MATCH syntax (bare words AND-joined, `"phrases"`, `OR`/`NOT`). Returns paths, titles, and snippet excerpts with `«match»` highlights ranked by BM25. Use as planner pre-flight: "have we touched this area before?" surfaces past designs and prior verdicts. Top-level files prefixed with `_` (e.g. `_lessons.md`) and bare `.md` files at the top of `tasks/` (legacy 0.6.x layout) are intentionally excluded. |
| `mmcg_history` | `query`, optional `kind`, `top` (default 10) | Searches `CONTEXT.md`, `CONTEXT-archive-*.md`, canonical task specs, executor reports, audits, `.mastermind/releases/*.md`, legacy task-local release notes, and lessons. `candidate` lessons are unresolved signals, not active guidance. Returns observed matches, `skipped_artifacts`, `truncated`, and an explicit retrieval-only epistemic contract. Markdown remains authoritative; re-index after Markdown changes. Each artifact is capped at 1 MiB and the corpus at 5,000 files. |
| `mmcg_dependency_cycles` | optional `language`, `min_size` (default 2) | Detect circular imports — strongly-connected components in the file-level import graph (Tarjan's algorithm). Each result is a cycle = a list of files. Pre-merge guard ("does this PR introduce a new cycle?") and architectural-hygiene survey. Resolves edges by leaf-name match — over-approximates (two unrelated `Logger` symbols cross-link) so verify before refactoring. Bump `min_size` to hide trivial A↔B and surface only larger structural problems. Work-capped at 50,000 file-pair edges: above that, `truncated: true` with an empty `cycles` list — incomplete and possibly inaccurate, not "more available"; narrow with `language` and retry. |
| `mmcg_symbols_changed_since` | `git_ref`, optional `root` | Symbol-level diff between a git ref and the current index. Returns `{added, removed, signature_changed}` symbol sets for files in `git diff --name-only <ref>..HEAD`. Re-parses old blobs from `git show <ref>:<path>` using the same extractor. Different from `mmcg_recent_changes` (watcher mtime) — this is git-ref-based, answering "what symbols did THIS PR/branch touch?". PR-review pre-flight, auditor verification, "what new public API appeared in v2.3?". Git subprocesses are killed after `MMCG_GIT_TIMEOUT_MS`; the per-file loop is capped at 10,000 files with `truncated: true` marking a partial diff. |
| `mmcg_status` | — | Index path, file/symbol counts, and bounded `stale_files`. A non-zero value means re-index before trusting structural answers. |

Tool responses are bounded JSON. Collection responses expose their own count or
collection metadata; status and workflow responses use named fields.

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
| `MMCG_QUERY_BUDGET_MS` | no | `10000` for `mmcg serve`; `60000` for `mmcg query`/`mmcg impact` | Wall-clock work budget for a single MCP tool call or CLI query. `0` = unlimited. See [Work budgets, timeouts, and cancellation](#work-budgets-timeouts-and-cancellation). |
| `MMCG_GIT_TIMEOUT_MS` | no | `30000` | Deadline for the `git` subprocesses behind `mmcg_symbols_changed_since` / `mmcg query symbols-changed-since`. A stuck `git` is killed and the call fails with a timeout error. |
| `MMCG_REQUEST_SOFT_TIMEOUT_MS` | no | `30000` | `mmcg serve` only. Wall-clock ceiling after which the watchdog cancels the in-flight request. `0` = no soft ceiling. See [Work budgets, timeouts, and cancellation](#work-budgets-timeouts-and-cancellation). |
| `MMCG_REQUEST_HARD_TIMEOUT_MS` | no | `300000` | `mmcg serve` only. Wall-clock ceiling after which the watchdog exits the process rather than let it keep burning CPU. `0` = no hard ceiling. |
| `MMCG_WATCHDOG` | no | unset | Set to `0` to disable the serve watchdog entirely — both ceilings and the reparent check. |

## Work budgets, timeouts, and cancellation

A single pathological query — a dense name-collision graph, a huge scoped
`mmcg_map`, a stuck `git` subprocess — used to be able to run for hours and
wedge every subsequent call in the session. Two independent mechanisms bound
that now: a **work budget** enforced inside SQLite, and a **watchdog** that
bounds the request as a whole.

The distinction matters. The work budget is a SQLite progress handler, so it
can only interrupt work that is executing *inside* SQLite. A handler that
spends its time in a Rust graph walk, an indexing pass, or waiting on a `git`
subprocess never reaches it. The watchdog is what covers those paths.

- **MCP serve** installs `MMCG_QUERY_BUDGET_MS` (default 10,000 ms; `0` =
  unlimited) around every `tools/call` dispatch, before the handler runs.
- **CLI queries** (`mmcg query <kind>`) use the same env var with a 60,000 ms
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
- **Git subprocesses** (`run_git`, `git_show_blob` behind
  `mmcg_symbols_changed_since`) are killed if they exceed
  `MMCG_GIT_TIMEOUT_MS` (default 30,000 ms). The per-file diff loop is
  additionally capped at 10,000 files; beyond that, `truncated: true` marks
  the response as a prefix, not the full diff.
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
  response reports `truncated: true` with an empty `cycles` list — SCC
  analysis is skipped entirely rather than run on a partial graph, because a
  capped cycle detector can split or hide real cycles. This is "incomplete
  and possibly inaccurate", never just "more available"; narrow with
  `language` and retry.

## Limitations (honest)

- **Ten file types.** Python (`.py`), TS/TSX (`.ts`, `.tsx`), JS/JSX (`.js`, `.jsx`, `.mjs`, `.cjs`), Vue SFC (`.vue`), Rust (`.rs`), C# (`.cs`), Go (`.go`), Java (`.java`), PHP (`.php`, `.phtml`), C/C++ (`.c`, `.cc`, `.cpp`, `.cxx`, `.h`, `.hpp`, `.hh`, `.hxx`, `.ipp`, `.tpp`). Other extensions are silently skipped.
- **Call resolution is name-based, not type-based.** `obj.foo()` records a call to "foo" with literal path "obj.foo" — but `obj` isn't resolved to a type, so cross-file precision is best-effort.
- **No cross-file symbol resolution.** Edges store `to_name` and `to_path` (strings), not `to_id`. Searching "callers of foo" finds *every* call to "foo" anywhere — even unrelated ones in different modules. The `to_path` column reduces ambiguity for imports specifically (use `mmcg_imported_by` with `match: path`).
- **Path-based queries reflect literal source text, not semantic FQN.** When you index code with `use foo::bar::Baz` and `mmcg_imported_by --query "foo::bar::Baz" --match path` you'll get that file. But if another file imports the same type via `use crate::Baz` (re-exported), it stores `to_path = "crate::Baz"` and won't match the FQN query. Resolving paths to canonical FQNs would require parsing `Cargo.toml` / `package.json` / `pyproject.toml` and following re-exports — that's compiler-level work and out of scope. **Use `match: name` for "find all consumers"** (catches everything regardless of import phrasing); reserve `match: path` for "find this exact import spelling".
- **`to_type` for member-call detection uses a capital-letter heuristic in TS/JS/Python.** `Class.method()`, `JSON.parse()`, `Foo.bar.baz()` correctly emit `to_type = "Class"` / `"JSON"` / `"Foo"` (rightmost capitalized receiver). For Rust, `to_type` is unambiguous via the `scoped_identifier` AST node — no heuristic needed. The heuristic misses calls on uppercase-named variables (e.g. `const FOO = new Bar(); FOO.method()` won't emit `to_type=FOO`) but matches the common convention used in idiomatic JS/TS/Python code.
- **JSX component usage is resolved by the uppercase-tag convention.** `<Button />` and `<Select.Option />` emit `calls` edges from the containing component; `<div>` and `<section>` do not, because a syntactic parser cannot otherwise tell a component from a host element. A component deliberately named in lowercase is invisible, and a capitalized host element in a custom renderer would be counted. `mmcg_callers` on a component therefore answers "who renders this", which is the frontend equivalent of "who calls this".
- **Variable-declared functions are named after their binding.** `const Card = () => …`, `const Card = function () {…}`, and one level of wrapper call (`memo(…)`, `forwardRef(…)`, `styled(…)`) become `function` symbols named `Card`, with the inner parameters as the signature so a changed props contract still reports as `signature_changed`. Wrapper unwrapping stops at one level: `memo(forwardRef(() => …))` finds no inner function and produces no symbol. A non-function value that merely receives a callback (`const x = compute(() => 1)`) is captured as a function symbol — a known over-capture of the same heuristic.
- **A Vue SFC is indexed as one component named after its file.** `MyCard.vue` defines `MyCard` (kind `component`), owning every symbol from its `<script>` block with `.vue`-absolute line numbers — the script is re-parsed with the real TypeScript or JavaScript grammar, chosen by `lang`, so it is not a weaker approximation. Template usage emits `calls` edges from the component. Both `<my-widget />` and `<MyWidget />` normalize to `MyWidget`, matching Vue's own resolution, so a search for the kebab spelling finds nothing. What stays invisible: expressions inside template attributes, and components auto-imported by build tooling rather than by an `import` statement.
- **`disciplines` classifies only what a path can establish.** `mmcg_change_impact` reports `frontend` for component and stylesheet file types (`.tsx`, `.jsx`, `.vue`, `.css`, `.scss`, `.sass`, `.less`), `qa` for the same test-file naming convention the test projection already uses, and `migration` for a `.sql` file or a path component named `migrations`, `migration`, or `migrate` — which covers the Django, Rails, Prisma, and Flyway layouts without guessing at a repository that keeps them elsewhere. A detected `migration` is a strict-mode trigger, not a statement about whether the migration is destructive. A file can belong to both. Everything else is returned under `unclassified` rather than guessed — a queue consumer, a migration, or an auth boundary in a plain `.ts` file has no path-level signal, and inventing one would be a confident answer without evidence. The block proposes an evidence set for routing; it is not a statement about what the change does.
- **Watcher doesn't index brand-new directories deeply on the first event.** If you `mkdir -p foo/bar/baz` and add files inside, the watcher catches each file event individually. No special handling — just slower for big batch additions.
- **Schema migration is destructive.** When mmcg's schema version changes (v1→v2, etc.) the existing index is dropped on open. Re-run `mmcg index .` to repopulate. No data is lost — the source code is the source of truth.
- **`mmcg_unreferenced` filters known framework patterns since 0.7.0.** The query excludes symbols decorated with pytest fixtures / parametrize / mark, web-framework route decorators (`.route` / `.get` / `.post` / `.put` / `.delete` / `.patch` / `.websocket`), JIT decorators (`triton.jit` / `numba.jit` / `nb.njit`), task queues (`celery.task` / `shared_task`), CLI (`click.command` / `click.group`), Rust attributes (`#[test]`, `#[tokio::main]`, `#[async_std::test]`), C# test/web/benchmark attributes (xUnit `[Fact]`/`[Theory]`, NUnit `[Test]`/`[SetUp]`, MSTest `[TestMethod]`, ASP.NET `[HttpGet]`/`[HttpPost]`/`[Route]`, BenchmarkDotNet `[Benchmark]`), and (since 0.9.0) Java/PHP frameworks: JUnit `@Test`/`@ParameterizedTest`/`@BeforeEach`, Spring `@GetMapping`/`@PostMapping`/`@Bean`/`@Scheduled`, PHPUnit `#[Test]`/`#[DataProvider]`, Symfony `#[Route]`/`#[AsCommand]`, Livewire `#[On]`. Also filters `test_*` functions in `*test*` / `*spec*` paths (pytest convention). Remaining false-positive classes that mmcg can't see: (a) entry points like `main` or framework-registered handlers without a recognized decorator; (b) dynamic dispatch — trait objects, duck-typed calls, JS reflection; (c) cross-language calls (TS subprocess invoking Python); (d) runtime registration via dict / list (`HANDLERS = {"foo": foo}`); (e) gtest-style C++ macro tests (`TEST(Suite, Name)` — macros invisible to tree-sitter); (f) Go test functions (`TestXxx(*testing.T)` convention — `testing` import is the closest signal, but we don't filter on imports). Review every hit manually before deleting. The tool is "candidates to investigate", not "safe to delete".
- **`mmcg_api_surface` is empirical, not declared.** It returns symbols that are *currently* called from outside the prefix — independent of language-level visibility (`pub`/`export`/no-underscore). A symbol declared `pub` with no external callers won't appear; a private symbol leaked through a public re-export and called externally will. Useful for "what does the rest of the codebase actually rely on?", not for "what's the public API contract?".
- **`mmcg_recent_changes` reflects index mtime, not git history.** If you re-index after rewriting history (rebase, amend, force-push) every touched file appears as "recent". Use `git log --since=...` for git truth; use this tool for "what has my watcher seen lately".
- **C# partial classes** are stored as one symbol per file (each declaration is a real top-level node in its own file). `mmcg_search` collapses them by default into a single hit with a `locations` array of every declaration — set `collapse_partials: false` to opt out. `mmcg_callers` / `mmcg_callees` / `mmcg_impact` / `mmcg_outline` are unaffected: they resolve by name and don't double-count. Non-partial classes with colliding names across namespaces are *not* collapsed (they're genuinely distinct).
- **Python module-level constants** (`MAX_RETRIES = 5`, `__all__ = [...]`, `HOST, PORT = ...`) are captured as `kind="constant"` since 0.14.0. Scoping is strict — only DIRECT children of the module node count; assignments inside `if` / `for` / `try` / class bodies / function bodies are not constants. `mmcg_unreferenced` **excludes constants by default** because the call/import graph doesn't track value-reads — every constant would otherwise appear as dead. Opt-in with `kind=constant` to surface unused constants explicitly. `mmcg_callers MAX_RETRIES` still works for the cases where a constant is referenced via `import` (`from foo import MAX_RETRIES` produces an `imports` edge) or attribute access (`foo.MAX_RETRIES` produces a `calls` edge to leaf `MAX_RETRIES`). Other languages: not extracted (TS/Rust/Go/etc. constants are typed declarations through different AST shapes — file a request if you need them).
- **C/C++ is best-effort.** The C/C++ extractor uses tree-sitter alone — no preprocessor, no template instantiation, no semantic analysis. Concretely: (a) **macros are invisible** — `TEST(Suite, Name) { ... }` is seen as a call to `TEST`, not as a function definition, so gtest/Catch2 test bodies don't appear in `mmcg_search` and calls inside macro arguments may be lost; (b) **templates** record the template name but not instantiations (`vector<int>` doesn't create a `vector<int>` symbol); (c) **header/source split** produces two symbol rows (`void Foo::bar()` declared in `.h` and defined in `.cpp` = two `bar` hits); (d) **ADL/overload resolution** isn't performed (`swap(a, b)` records `swap` without knowing which namespace it resolves to); (e) **`#include`** records the header as an `imports` edge but doesn't follow its contents. For high-precision C++ structural analysis use `clangd` (semantic, slow, large) or `ctags` (similar tradeoffs to this extractor). mmcg uses one `tree-sitter-cpp` grammar for both `.c` and `.cpp` files — rare C-only code that uses C++ keywords as identifiers (e.g. a variable named `new`) may mis-parse.

## CI

`mastermind ci` indexes the repository, verifies selected specs, parses their
executor reports, audits the real diff, and optionally emits sealed bundles.
For pull requests, scope the gate to changed task folders and require evidence:

```bash
mastermind ci --since origin/main \
  --changed-only --require-executor-report \
  --bundle-dir .mastermind/audit-output
```

Without `--changed-only`, the command retains its compatibility behavior and
walks all task specs. Bundle publication always requires a canonical
`executor-report.md`, even if the explicit requirement flag is omitted.

### Schema-v3 audit envelopes

`mastermind audit-spec ... --bundle evidence.json` seals the mechanical report as canonical JSON with a manifest SHA-256 digest. A valid digest is tamper-evidence, not provenance: verification succeeds only with a complete exact repository/baseline/head/root/clean-worktree policy, a required Ed25519 signature rooted in an allowlisted non-revoked key ID, or both. `mastermind audit verify ... --integrity-only` is labelled untrusted and reports authenticity and policy as `not_evaluated`.

Detached signatures use a domain-separated schema-v1 statement binding the envelope schema, hash algorithm, canonicalization, key ID, and manifest digest. Private keys are single-line base64 32-byte Ed25519 seeds with Unix mode 0600. Key owners are responsible for rotation, revocation allowlists, and protecting historical verification policy; Ed25519 alone provides neither signing time nor pre/post-compromise distinction.

The Docker Action requires exact full baseline and head OIDs, a GitHub `owner/repo`, and a clean worktree. Its publication example treats `workflow_run` artifacts as hostile until independent run, attempt, workflow blob, PR, artifact ID/digest/size, envelope, and policy checks pass. The resulting attestation means only publication-workflow verification provenance; it does not prove that PR analysis ran in a trusted environment or that the findings are true.

`.github/workflows/ci-mmcg.yml` runs the full test suite plus an end-to-end smoke (`mmcg doctor --json` + `mmcg verify-spec` + `mmcg audit-spec` against `tests/ci-fixture/`) on a 6-target matrix every PR: x86_64/aarch64 Linux gnu + musl, aarch64 macOS (Apple Silicon), x86_64 Windows. macOS Intel (`x86_64-apple-darwin`) is not gated per-PR but builds locally via `cargo install --target=x86_64-apple-darwin`.
