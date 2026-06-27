---
name: mmcg
description: Mastermind Codegraph — fast multi-language code indexer (Python + TypeScript/TSX + JavaScript/JSX + Rust + C# + Go + Java + PHP + C/C++) exposed over MCP. Indexes symbols, calls, and imports (with fully-qualified paths) into a local SQLite database (`.mastermind/mmcg.db`) and exposes 20 structural query tools for AI agents in the Mastermind workflow. Includes FTS5 search over `.mastermind/tasks/` and an incremental file watcher.
metadata:
  version: 0.35.0
  authors:
    - mastermind
  tags:
    - mmcg
    - codegraph
    - python
    - typescript
    - javascript
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

A small, fast Rust binary that builds a structural index of a codebase (Python, TypeScript/TSX, JavaScript/JSX, Rust, C#, Go, Java, PHP, C/C++). It serves the graph over **MCP** (20 read-only tools), but MCP is one surface — the same binary is the Mastermind workflow's **CLI**: the deterministic spec gates (`verify-spec` / `audit-spec`), project setup (`init` / `doctor`), and miners (`miner profile`, which learns your code-shape style into `~/.mastermind/style.md`) are all subcommands of it. Pair with the workflow so the planner/executor reason from a real graph instead of grep heuristics.

> Installed via npm (`@xcraftmind/mastermind`)? The command is **`mastermind`** — the same binary. The `mmcg` name used throughout this doc is the cargo-installed alias (`cargo install mmcg`).

## What it indexes

For each supported file, mmcg captures:

| Construct | Python | TS/JS | Rust | C# | Go | Java | PHP | C/C++ |
|---|---|---|---|---|---|---|---|---|
| Functions | ✓ `def` | ✓ `function` | ✓ `fn` | ✓ method/local-fn | ✓ `func` | ✓ `method_declaration` | ✓ `function` | ✓ `function_definition` |
| Module constants | ✓ `FOO = ...` (direct module children only) | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| Methods | ✓ inside classes | ✓ inside classes | ✓ inside `impl`/`trait` | ✓ inside classes/etc | ✓ `func (r T) M()` | ✓ inside classes | ✓ inside classes | ⚠️ inside classes + `T::m()` def |
| Types | ✓ `class` | ✓ `class`, `interface` | ✓ `struct`, `enum`, `trait` | ✓ `class`, `struct`, `interface`, `record`, `enum` | ✓ `struct`, `interface` | ✓ `class`, `interface`, `enum`, `record` | ✓ `class`, `interface`, `trait`, `enum` | ✓ `class`, `struct`, `union`, `enum` |
| Calls | ✓ | ✓ | ✓ + `Mod::foo()` | ✓ + `Type.Method()` | ✓ + `pkg.Func()` + `Foo{}` | ✓ + `new Foo()` | ✓ + `Foo::bar()` + `new Foo` | ⚠️ + `new Foo` (no template inst) |
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
| TypeScript | function, arrow-fn, method, class, interface, type-alias | ✓ + `new Foo()` constructors + method calls | ✓ ES named / default / namespace + re-exports | anonymous default exports lose name; `export * from` re-exports not expanded to member level |
| JavaScript | function, arrow-fn, method, class | ✓ same walker as TS (TS-only node kinds skip silently) | ✓ ES named / default / namespace | no CommonJS `require()` as import edge; same gaps as TS minus interface / type-alias |
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

**Skipped directories:** `.git`, `.mastermind`, `.venv`, `venv`, `__pycache__`, `node_modules`, `target`, `dist`, `build`, `.tox`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.next`, `.turbo`, `.cache`.

## Why a custom indexer

The Mastermind workflow needs structural queries every few seconds (planner deciding blast radius, executor checking callers before edits). Grep/Read each costs hundreds of tokens; mmcg returns the same info in dozens. Multiplied across a workflow run, the difference is between affordable and not.

mmcg is intentionally narrow:
- Nine languages (Python, TypeScript/TSX, JavaScript/JSX, Rust, C#, Go, Java, PHP, C/C++) — extend by adding a parser, not by depending on multi-language toolchains
- 20 query tools that map directly to the workflow's needs
- Read-only over MCP (no writes from agents — only `mmcg index` and `mmcg watch` mutate the db)

## Speed notes

- **Parsers**: tree-sitter (C, vendored — no system tree-sitter required)
- **Parallelism**: `rayon` parses files in parallel; writes serialize through a single SQLite connection (WAL mode)
- **Batching**: one transaction per file via `Store::commit_file` — prepared statements cached
- **Storage**: SQLite with indexes on `symbols.name`, `edges.from_id`, `edges.to_name`. Sub-millisecond queries on real codebases.

On the mmcg crate itself (10 Rust files, 1 Python script): indexing takes **9 ms** for 186 symbols + 1294 edges.

## Build from source

Requires Rust 1.75+ ([rustup](https://rustup.rs/)). No system libraries — SQLite and tree-sitter are bundled at compile time.

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
# Diffs against <git-ref> (typically `main` or merge-base). Flags scope creep,
# pre-edit snapshot drift, vanished symbols. Exit 1 if verdict is `broken`.
mmcg audit-spec .mastermind/tasks/042-feature/spec.md --since main

# Two-phase orchestrator: deterministic shell around the executor.
#   1st invocation → verify-spec + risk report + write state
#                    (.mastermind/run-state/<spec>.json), captures git HEAD as baseline
#   2nd invocation (after executor) → audit-spec vs baseline; on Held verdict, emit
#                    release-notes draft to stdout AND .mastermind/releases/<spec>.md;
#                    clear state. Drift/Broken keeps state for retry after fixes.
mmcg run-task .mastermind/tasks/042-feature/spec.md             # hand-off semantics
mmcg run-task .mastermind/tasks/042-feature/spec.md --exec      # shell out to `claude -p` between phases
mmcg run-task .mastermind/tasks/042-feature/spec.md --reset     # drop state, force pre-flight (counter survives)
mmcg run-task .mastermind/tasks/042-feature/spec.md --pre-only  # never auto-resume into post
mmcg run-task .mastermind/tasks/042-feature/spec.md --post-only # requires state
mmcg run-task .mastermind/tasks/042-feature/spec.md --allow-no-index  # docs-only / spec-only specs
mmcg run-task .mastermind/tasks/042-feature/spec.md --strict          # fold strict spec checks into pre-flight
mmcg run-task .mastermind/tasks/042-feature/spec.md --max-iterations 5 # raise the default budget (default 3)
mmcg run-task .mastermind/tasks/042-feature/spec.md --force-iteration  # bypass budget for this attempt; auto-lesson still fires
# NOTE: without --allow-no-index, pre-flight hard-fails when the index is missing
# or empty. Gates without a codegraph degrade to file-existence + section checks
# only — mmcg's value comes from the structural truth layer, not the heuristics.

# Initialize a project: scaffold .mastermind/, build the index, and draft
# CONTEXT.md from the codebase via `claude -p`. Use --no-index / --no-claude to
# skip those. --profile pre-seeds a stack-specific CONTEXT.md (conventions,
# test/lint commands, canonical gotchas).
mmcg init                              # generic CONTEXT.md (default)
mmcg init --profile typescript-api    # Node.js HTTP/REST/GraphQL API
mmcg init --profile react-native      # mobile (Expo or bare)
mmcg init --profile python-fastapi    # async Python API
mmcg init --profile rust-cli          # command-line tool

# Register mmcg with Claude Code's MCP layer. Safe by default — prints a diff
# and exits without writing unless `--write-mcp` is passed. Merges into existing
# `mcpServers` (preserves other servers). Refuses to overwrite a customized
# mmcg entry without `--force`.
mmcg setup claude                                         # dry-run: show the `claude mcp add` it would run
mmcg setup claude --write-mcp                             # register at user scope (~/.claude.json) via `claude mcp add`
mmcg setup claude --project . --write-mcp                 # write ./.mcp.json (project scope)
mmcg setup claude --project . --write-mcp --with-workflow # also drop CLAUDE.md workflow template
mmcg setup claude --write-mcp --force                     # overwrite a customized mmcg entry

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

- **mtime newer than stored** → re-parse and commit (counted as `indexed`)
- **mtime equals stored** → skip without parsing (counted as `unchanged`)
- **file in index but not on disk** → purge from index (counted as `purged`)
- **unsupported extension** → skip (counted as `skipped`)

Output example:

```
indexed 3 (unchanged 124, purged 1, failed 0) / scanned 1247 | 87 symbols | 412 edges | 84 ms
```

When to use `--force`:
- After a schema version change (the index is also dropped+rebuilt automatically on schema mismatch, but `--force` lets you force a rebuild without bumping schema)
- If you suspect the index is stale for reasons mtime can't see (e.g., a file was restored from backup with old mtime)
- For benchmarking — to see how long a cold index takes

**Orphan purge caveat.** mmcg purges any indexed path that was not seen during this run's walk. This is the right behavior when you run `mmcg index <same-root>` every time. But if you switch roots between runs (e.g. `mmcg index .` then `mmcg index src/`), the second run will see different paths and wrongly purge the rest.

**Best practice:** pick one project root (usually `.` from your project's top directory) and stick with it. `mmcg watch` always uses the root you pass at startup. If you accidentally indexed a different root, run `mmcg index --force <correct-root>` to rebuild from scratch.

The index lives at `.mastermind/mmcg.db` in the current directory by default. Override with `--index <path>` or env var `MMCG_INDEX_PATH`.

## MCP server usage

```bash
mmcg serve
```

For Claude Code, run `mastermind setup claude --write-mcp` instead of editing JSON by hand. For other MCP stdio clients, add:

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
| `mmcg_impact` | `name`, optional `max_depth` (1-10, default 2), `language` | Transitive callers via `calls` edges. Full blast radius. |
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
| `mmcg_tasks` | `query`, optional `top` (default 10) | Full-text search past task specs (`.mastermind/tasks/<NNN>-<name>/spec.md`). FTS5 MATCH syntax (bare words AND-joined, `"phrases"`, `OR`/`NOT`). Returns paths, titles, and snippet excerpts with `«match»` highlights ranked by BM25. Use as planner pre-flight: "have we touched this area before?" surfaces past designs and prior verdicts. Top-level files prefixed with `_` (e.g. `_lessons.md`) and bare `.md` files at the top of `tasks/` (legacy 0.6.x layout) are intentionally excluded. |
| `mmcg_dependency_cycles` | optional `language`, `min_size` (default 2) | Detect circular imports — strongly-connected components in the file-level import graph (Tarjan's algorithm). Each result is a cycle = a list of files. Pre-merge guard ("does this PR introduce a new cycle?") and architectural-hygiene survey. Resolves edges by leaf-name match — over-approximates (two unrelated `Logger` symbols cross-link) so verify before refactoring. Bump `min_size` to hide trivial A↔B and surface only larger structural problems. |
| `mmcg_symbols_changed_since` | `git_ref`, optional `root` | Symbol-level diff between a git ref and the current index. Returns `{added, removed, signature_changed}` symbol sets for files in `git diff --name-only <ref>..HEAD`. Re-parses old blobs from `git show <ref>:<path>` using the same extractor. Different from `mmcg_recent_changes` (watcher mtime) — this is git-ref-based, answering "what symbols did THIS PR/branch touch?". PR-review pre-flight, auditor verification, "what new public API appeared in v2.3?". |
| `mmcg_status` | — | Index health (file count, symbol count, db path). |

All responses are JSON with a `count` field for quick scanning before reading detail.

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

## Limitations (honest)

- **Nine languages.** Python (`.py`), TS/TSX (`.ts`, `.tsx`), JS/JSX (`.js`, `.jsx`, `.mjs`, `.cjs`), Rust (`.rs`), C# (`.cs`), Go (`.go`), Java (`.java`), PHP (`.php`, `.phtml`), C/C++ (`.c`, `.cc`, `.cpp`, `.cxx`, `.h`, `.hpp`, `.hh`, `.hxx`, `.ipp`, `.tpp`). Other extensions are silently skipped.
- **Call resolution is name-based, not type-based.** `obj.foo()` records a call to "foo" with literal path "obj.foo" — but `obj` isn't resolved to a type, so cross-file precision is best-effort.
- **No cross-file symbol resolution.** Edges store `to_name` and `to_path` (strings), not `to_id`. Searching "callers of foo" finds *every* call to "foo" anywhere — even unrelated ones in different modules. The `to_path` column reduces ambiguity for imports specifically (use `mmcg_imported_by` with `match: path`).
- **Path-based queries reflect literal source text, not semantic FQN.** When you index code with `use foo::bar::Baz` and `mmcg_imported_by --query "foo::bar::Baz" --match path` you'll get that file. But if another file imports the same type via `use crate::Baz` (re-exported), it stores `to_path = "crate::Baz"` and won't match the FQN query. Resolving paths to canonical FQNs would require parsing `Cargo.toml` / `package.json` / `pyproject.toml` and following re-exports — that's compiler-level work and out of scope. **Use `match: name` for "find all consumers"** (catches everything regardless of import phrasing); reserve `match: path` for "find this exact import spelling".
- **`to_type` for member-call detection uses a capital-letter heuristic in TS/JS/Python.** `Class.method()`, `JSON.parse()`, `Foo.bar.baz()` correctly emit `to_type = "Class"` / `"JSON"` / `"Foo"` (rightmost capitalized receiver). For Rust, `to_type` is unambiguous via the `scoped_identifier` AST node — no heuristic needed. The heuristic misses calls on uppercase-named variables (e.g. `const FOO = new Bar(); FOO.method()` won't emit `to_type=FOO`) but matches the common convention used in idiomatic JS/TS/Python code.
- **Watcher doesn't index brand-new directories deeply on the first event.** If you `mkdir -p foo/bar/baz` and add files inside, the watcher catches each file event individually. No special handling — just slower for big batch additions.
- **Schema migration is destructive.** When mmcg's schema version changes (v1→v2, etc.) the existing index is dropped on open. Re-run `mmcg index .` to repopulate. No data is lost — the source code is the source of truth.
- **`mmcg_unreferenced` filters known framework patterns since 0.7.0.** The query excludes symbols decorated with pytest fixtures / parametrize / mark, web-framework route decorators (`.route` / `.get` / `.post` / `.put` / `.delete` / `.patch` / `.websocket`), JIT decorators (`triton.jit` / `numba.jit` / `nb.njit`), task queues (`celery.task` / `shared_task`), CLI (`click.command` / `click.group`), Rust attributes (`#[test]`, `#[tokio::main]`, `#[async_std::test]`), C# test/web/benchmark attributes (xUnit `[Fact]`/`[Theory]`, NUnit `[Test]`/`[SetUp]`, MSTest `[TestMethod]`, ASP.NET `[HttpGet]`/`[HttpPost]`/`[Route]`, BenchmarkDotNet `[Benchmark]`), and (since 0.9.0) Java/PHP frameworks: JUnit `@Test`/`@ParameterizedTest`/`@BeforeEach`, Spring `@GetMapping`/`@PostMapping`/`@Bean`/`@Scheduled`, PHPUnit `#[Test]`/`#[DataProvider]`, Symfony `#[Route]`/`#[AsCommand]`, Livewire `#[On]`. Also filters `test_*` functions in `*test*` / `*spec*` paths (pytest convention). Remaining false-positive classes that mmcg can't see: (a) entry points like `main` or framework-registered handlers without a recognized decorator; (b) dynamic dispatch — trait objects, duck-typed calls, JS reflection; (c) cross-language calls (TS subprocess invoking Python); (d) runtime registration via dict / list (`HANDLERS = {"foo": foo}`); (e) gtest-style C++ macro tests (`TEST(Suite, Name)` — macros invisible to tree-sitter); (f) Go test functions (`TestXxx(*testing.T)` convention — `testing` import is the closest signal, but we don't filter on imports). Review every hit manually before deleting. The tool is "candidates to investigate", not "safe to delete".
- **`mmcg_api_surface` is empirical, not declared.** It returns symbols that are *currently* called from outside the prefix — independent of language-level visibility (`pub`/`export`/no-underscore). A symbol declared `pub` with no external callers won't appear; a private symbol leaked through a public re-export and called externally will. Useful for "what does the rest of the codebase actually rely on?", not for "what's the public API contract?".
- **`mmcg_recent_changes` reflects index mtime, not git history.** If you re-index after rewriting history (rebase, amend, force-push) every touched file appears as "recent". Use `git log --since=...` for git truth; use this tool for "what has my watcher seen lately".
- **C# partial classes** are stored as one symbol per file (each declaration is a real top-level node in its own file). `mmcg_search` collapses them by default into a single hit with a `locations` array of every declaration — set `collapse_partials: false` to opt out. `mmcg_callers` / `mmcg_callees` / `mmcg_impact` / `mmcg_outline` are unaffected: they resolve by name and don't double-count. Non-partial classes with colliding names across namespaces are *not* collapsed (they're genuinely distinct).
- **Python module-level constants** (`MAX_RETRIES = 5`, `__all__ = [...]`, `HOST, PORT = ...`) are captured as `kind="constant"` since 0.14.0. Scoping is strict — only DIRECT children of the module node count; assignments inside `if` / `for` / `try` / class bodies / function bodies are not constants. `mmcg_unreferenced` **excludes constants by default** because the call/import graph doesn't track value-reads — every constant would otherwise appear as dead. Opt-in with `kind=constant` to surface unused constants explicitly. `mmcg_callers MAX_RETRIES` still works for the cases where a constant is referenced via `import` (`from foo import MAX_RETRIES` produces an `imports` edge) or attribute access (`foo.MAX_RETRIES` produces a `calls` edge to leaf `MAX_RETRIES`). Other languages: not extracted (TS/Rust/Go/etc. constants are typed declarations through different AST shapes — file a request if you need them).
- **C/C++ is best-effort.** The C/C++ extractor uses tree-sitter alone — no preprocessor, no template instantiation, no semantic analysis. Concretely: (a) **macros are invisible** — `TEST(Suite, Name) { ... }` is seen as a call to `TEST`, not as a function definition, so gtest/Catch2 test bodies don't appear in `mmcg_search` and calls inside macro arguments may be lost; (b) **templates** record the template name but not instantiations (`vector<int>` doesn't create a `vector<int>` symbol); (c) **header/source split** produces two symbol rows (`void Foo::bar()` declared in `.h` and defined in `.cpp` = two `bar` hits); (d) **ADL/overload resolution** isn't performed (`swap(a, b)` records `swap` without knowing which namespace it resolves to); (e) **`#include`** records the header as an `imports` edge but doesn't follow its contents. For high-precision C++ structural analysis use `clangd` (semantic, slow, large) or `ctags` (similar tradeoffs to this extractor). mmcg uses one `tree-sitter-cpp` grammar for both `.c` and `.cpp` files — rare C-only code that uses C++ keywords as identifiers (e.g. a variable named `new`) may mis-parse.

## CI

`.github/workflows/ci-mmcg.yml` runs the full test suite plus an end-to-end smoke (`mmcg doctor --json` + `mmcg verify-spec` + `mmcg audit-spec` against `tests/ci-fixture/`) on a 6-target matrix every PR: x86_64/aarch64 Linux gnu + musl, aarch64 macOS (Apple Silicon), x86_64 Windows. macOS Intel (`x86_64-apple-darwin`) is not gated per-PR but builds locally via `cargo install --target=x86_64-apple-darwin`.
