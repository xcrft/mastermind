---
name: mmcg
description: Mastermind Codegraph — fast multi-language code indexer (Python + TypeScript/TSX + JavaScript/JSX + Rust) exposed over MCP. Indexes symbols, calls, and imports (with fully-qualified paths) into a local SQLite database (`.mastermind/mmcg.db`) and exposes 12 structural query tools for AI agents in the Mastermind workflow. Includes an incremental file watcher.
metadata:
  version: 0.7.0
  authors:
    - mastermind
  tags:
    - mmcg
    - codegraph
    - python
    - typescript
    - javascript
    - rust
  transport: stdio
  source: this repository
---

# mmcg — Mastermind Codegraph

A small, fast Rust binary that builds a structural index of a codebase (Python, TypeScript/TSX, JavaScript/JSX, Rust) and serves queries over MCP. Pair with the Mastermind workflow so the planner/executor reason from a real graph instead of grep heuristics.

**Status — Phase 6:** Python + TypeScript + JavaScript + Rust. Imports tracked with fully-qualified paths. File watcher. Incremental indexing. **Language filter** on all queries (defends against monorepo name collisions). **Rust type-method awareness** — `mmcg_callers SessionStore` now finds `SessionStore::new()` callers. **`mmcg_symbols_in_file`** — list all symbols in a file in source order. **Phase 6 additions (0.6.0):** `mmcg_outline` (symbol tree per file), `mmcg_recent_changes` (files re-indexed within a time window), `mmcg_unreferenced` (dead-code candidates), `mmcg_api_surface` (symbols referenced from outside a prefix), and an `edge_kind` filter on `mmcg_callers` / `mmcg_callees` (switch between `calls` / `imports` / `inherits`).

## What it indexes

For each supported file, mmcg captures:

| Construct | Python | TS / TSX | JS / JSX | Rust |
|---|---|---|---|---|
| Top-level functions | ✓ `def`, `async def` | ✓ `function` | ✓ `function` | ✓ `fn` |
| Classes | ✓ | ✓ `class`, `interface`, `abstract class` | ✓ `class` | ✓ `struct`, `enum`, `trait` |
| Methods | ✓ inside classes | ✓ inside classes/interfaces | ✓ inside classes | ✓ inside `impl` / `trait` blocks |
| Calls | ✓ `foo()`, `obj.foo()` | ✓ `foo()`, `obj.foo()`, `new Foo()` | ✓ same as TS | ✓ `foo()`, `obj.foo()`, `Mod::foo()` |
| Macro invocations | n/a | n/a | n/a | ✓ `println!`, `vec![]` |
| Imports | ✓ `import`, `from … import …` | ✓ default, named, namespace | ✓ same as TS | ✓ `use` with paths, lists, aliases, wildcards |
| Module-level calls | ✓ attributed to `<module>` symbol | ✓ | ✓ | ✓ |
| **Fully-qualified path** | ✓ `collections.abc.Iterable` | ✓ `'pkg'::default` / `'./mod'::name` | ✓ same as TS | ✓ `foo::bar::Baz` |

Each file gets a synthetic `<module>` symbol (kind `module`) that owns module-scope imports and top-level statements.

### Path format per language

- **Python:** dotted. `from collections.abc import Iterable as Iter` → name=`Iter`, path=`collections.abc.Iterable`
- **TS/JS:** module-source + leaf separated by `::`. `import { foo as bar } from './a'` → name=`bar`, path=`./a::foo`. Defaults: `<src>::default`. Namespace: `<src>::*`.
- **Rust:** Rust-style `::`. `use foo::bar::Baz as Q` → name=`Q`, path=`foo::bar::Baz`. Wildcards: `foo::*`.
- **Calls:** for `obj.foo()`, path is the literal `obj.foo` from source (no type resolution — see Limitations).

**Skipped directories:** `.git`, `.mastermind`, `.venv`, `venv`, `__pycache__`, `node_modules`, `target`, `dist`, `build`, `.tox`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.next`, `.turbo`, `.cache`.

## Why a custom indexer

The Mastermind workflow needs structural queries every few seconds (planner deciding blast radius, executor checking callers before edits). Grep/Read each costs hundreds of tokens; mmcg returns the same info in dozens. Multiplied across a workflow run, the difference is between affordable and not.

mmcg is intentionally narrow:
- Four languages (Python, TypeScript/TSX, JavaScript/JSX, Rust) — extend by adding a parser, not by depending on multi-language toolchains
- 12 query tools that map directly to the workflow's needs
- Read-only over MCP (no writes from agents — only `mmcg index` and `mmcg watch` mutate the db)

## Speed notes

- **Parsers**: tree-sitter (C, vendored — no system tree-sitter required)
- **Parallelism**: `rayon` parses files in parallel; writes serialize through a single SQLite connection (WAL mode)
- **Batching**: one transaction per file via `Store::commit_file` — prepared statements cached
- **Storage**: SQLite with indexes on `symbols.name`, `edges.from_id`, `edges.to_name`. Sub-millisecond queries on real codebases.

On the mmcg crate itself (10 Rust files, 1 Python script): indexing takes **9 ms** for 186 symbols + 1294 edges.

## Install

You need a Rust toolchain (1.75+):

```bash
cd mcp/servers/mmcg
cargo install --path .
```

This installs the `mmcg` binary into `~/.cargo/bin/`. No system libraries required — SQLite and tree-sitter are bundled into the binary at build time.

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

Add to your MCP client config (e.g. `~/.claude/mcp.json` or `claude_desktop_config.json`):

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

For best results, run `mmcg watch` in a separate terminal so the index stays current while you work.

## MCP tools exposed (12)

| Tool | Args | What it returns |
|---|---|---|
| `mmcg_search` | `name`, optional `kind`, `language` | Symbols matching exactly. Pre-flight "does X exist?" check. Returns location, kind, and signature. |
| `mmcg_callers` | `name`, optional `language`, `edge_kind` (default `calls`) | **Containing functions** that reference `name` by the given edge kind. Count = distinct containing units, not distinct call sites (a function with 3 calls to `name` counts once). Use before editing for blast radius. Pass `edge_kind: imports` to find importers via the same tool. |
| `mmcg_callees` | `name`, optional `language`, `edge_kind` (default `calls`) | Names the symbol references via the given edge kind. |
| `mmcg_impact` | `name`, optional `max_depth` (1-10, default 2), `language` | Transitive callers via `calls` edges. Full blast radius. |
| `mmcg_imports` | `file` | Names imported by this file's top-level imports — each entry has `name`, `path` (fully-qualified), `line`. |
| `mmcg_imported_by` | `query`, optional `match: name`(default)/`path`, `language` | Files whose imports reference the given name OR fully-qualified path. Use `match: path` when the leaf name is ambiguous across modules. |
| `mmcg_symbols_in_file` | `file` | All symbols defined in a file, source order. Flat list. |
| `mmcg_outline` | `file` | Symbol tree of a file — classes/impls own their methods, modules own top-level functions. One call replaces a search + multiple lookups. |
| `mmcg_files` | optional `prefix`, `language` | Indexed files with symbol counts. |
| `mmcg_recent_changes` | `since` (e.g. `2h`, `30m`, `1d`) | Files re-indexed within the given window. Useful for "what changed recently?" during incident investigation. |
| `mmcg_unreferenced` | optional `kind`, `language` | Symbols that no edge references. Dead-code candidates. **Review manually** — see Limitations for false-positive scenarios. |
| `mmcg_api_surface` | `prefix`, optional `language` | Symbols under `prefix` referenced from at least one file OUTSIDE `prefix`. Empirical "who-uses-this-module" map; doesn't need declared visibility. |
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

- **Four languages.** Python (`.py`), TS/TSX (`.ts`, `.tsx`), JS/JSX (`.js`, `.jsx`, `.mjs`, `.cjs`), Rust (`.rs`). Other extensions are silently skipped.
- **Call resolution is name-based, not type-based.** `obj.foo()` records a call to "foo" with literal path "obj.foo" — but `obj` isn't resolved to a type, so cross-file precision is best-effort.
- **No cross-file symbol resolution.** Edges store `to_name` and `to_path` (strings), not `to_id`. Searching "callers of foo" finds *every* call to "foo" anywhere — even unrelated ones in different modules. The `to_path` column reduces ambiguity for imports specifically (use `mmcg_imported_by` with `match: path`).
- **Path-based queries reflect literal source text, not semantic FQN.** When you index code with `use foo::bar::Baz` and `mmcg_imported_by --query "foo::bar::Baz" --match path` you'll get that file. But if another file imports the same type via `use crate::Baz` (re-exported), it stores `to_path = "crate::Baz"` and won't match the FQN query. Resolving paths to canonical FQNs would require parsing `Cargo.toml` / `package.json` / `pyproject.toml` and following re-exports — that's compiler-level work and out of scope. **Use `match: name` for "find all consumers"** (catches everything regardless of import phrasing); reserve `match: path` for "find this exact import spelling".
- **`to_type` for member-call detection uses a capital-letter heuristic in TS/JS/Python.** `Class.method()`, `JSON.parse()`, `Foo.bar.baz()` correctly emit `to_type = "Class"` / `"JSON"` / `"Foo"` (rightmost capitalized receiver). For Rust, `to_type` is unambiguous via the `scoped_identifier` AST node — no heuristic needed. The heuristic misses calls on uppercase-named variables (e.g. `const FOO = new Bar(); FOO.method()` won't emit `to_type=FOO`) but matches the common convention used in idiomatic JS/TS/Python code.
- **Watcher doesn't index brand-new directories deeply on the first event.** If you `mkdir -p foo/bar/baz` and add files inside, the watcher catches each file event individually. No special handling — just slower for big batch additions.
- **Schema migration is destructive.** When mmcg's schema version changes (v1→v2, etc.) the existing index is dropped on open. Re-run `mmcg index .` to repopulate. No data is lost — the source code is the source of truth.
- **`mmcg_unreferenced` filters known framework patterns since 0.7.0.** The query excludes symbols decorated with pytest fixtures / parametrize / mark, web-framework route decorators (`.route` / `.get` / `.post` / `.put` / `.delete` / `.patch` / `.websocket`), JIT decorators (`triton.jit` / `numba.jit` / `nb.njit`), task queues (`celery.task` / `shared_task`), CLI (`click.command` / `click.group`), and Rust attributes (`#[test]`, `#[tokio::main]`, `#[async_std::test]`). Also filters `test_*` functions in `*test*` / `*spec*` paths (pytest convention). Remaining false-positive classes that mmcg can't see: (a) entry points like `main` or framework-registered handlers without a recognized decorator; (b) dynamic dispatch — trait objects, duck-typed calls, JS reflection; (c) cross-language calls (TS subprocess invoking Python); (d) runtime registration via dict / list (`HANDLERS = {"foo": foo}`). Review every hit manually before deleting. The tool is "candidates to investigate", not "safe to delete".
- **`mmcg_api_surface` is empirical, not declared.** It returns symbols that are *currently* called from outside the prefix — independent of language-level visibility (`pub`/`export`/no-underscore). A symbol declared `pub` with no external callers won't appear; a private symbol leaked through a public re-export and called externally will. Useful for "what does the rest of the codebase actually rely on?", not for "what's the public API contract?".
- **`mmcg_recent_changes` reflects index mtime, not git history.** If you re-index after rewriting history (rebase, amend, force-push) every touched file appears as "recent". Use `git log --since=...` for git truth; use this tool for "what has my watcher seen lately".

## Files in this artifact

| Path | What it is |
|---|---|
| `Cargo.toml` | Rust deps (rusqlite-bundled, tree-sitter + 3 grammars, rayon, notify, clap, serde) |
| `config.json` | MCP server config snippet for client setup |
| `src/main.rs` | CLI entry (`index`, `serve`, `watch`, `status`, `query`) |
| `src/lib.rs` | Module facade |
| `src/store.rs` | SQLite schema, batched writes, all read queries |
| `src/indexer.rs` | Multi-language dispatch — `LanguageExtractor` trait + parallel file walk |
| `src/indexer/python.rs` | Python extractor |
| `src/indexer/typescript.rs` | TypeScript / TSX extractor (shared walker also used by JS) |
| `src/indexer/javascript.rs` | JavaScript / JSX extractor — delegates to TS walker |
| `src/indexer/rust_lang.rs` | Rust extractor (named to avoid `rust` keyword clash) |
| `src/queries.rs` | High-level query API with serializable response types |
| `src/mcp.rs` | JSON-RPC over stdio, MCP `initialize`/`tools/list`/`tools/call` |
| `src/watcher.rs` | notify-based filesystem watcher with debouncing |

Tests live in `#[cfg(test)]` blocks in each module. Run with `cargo test` — 15 tests covering schema, queries, all three extractors.

## Integration with the Mastermind workflow

This artifact is the **truth layer** for the `mastermind-workflow` — see that file for how planner/executor/researcher are taught to query mmcg first for code-structural questions. Integration wiring (skill updates that mandate codegraph-first lookups) lands in a follow-up round.
