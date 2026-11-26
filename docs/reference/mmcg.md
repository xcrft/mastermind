# mmcg CLI and index

`mmcg` builds a local structural index of a repository and answers queries over
it from the CLI, the MCP server and Lens. All three read the same SQLite index.
This page covers what gets indexed, the CLI commands, freshness and limits. Start
with [Getting started](../getting-started.md) if you have not installed it yet.

`mastermind` and `mmcg` are the same binary. The npm package installs both
names, a Cargo install provides only `mmcg`. Examples use `mastermind`, and
`mastermind COMMAND --help` lists every argument.

## Reference files

| You need | Read |
|---|---|
| Indexed languages, gaps and source rules | [What it indexes](#what-it-indexes) on this page |
| Commands, freshness, budgets and limits | [CLI usage](#cli-usage), [Work budgets](#work-budgets-timeouts-and-cancellation), [Limitations](#limitations) |
| MCP protocol, tools and their limits | [MCP server](mcp.md) |
| Natural-language symbol retrieval and source review | [Concept search](concept-search.md) |
| Local review UI, evidence overlays, review export and SARIF | [Lens and review export](lens.md) |
| Architecture rules | [Policy as code](policy.md) |
| Task gates, spec fields, executor report rules and CI | [Specs, reports and CI](spec-and-report.md) |
| Runner, native executor, reviewer and repair contracts | [Task runtime](task-runtime.md) |
| Declarative evidence from scanners and tests | [Fact-ingestion SDK](../fact-ingestion-sdk.md) |
| A graph across several local repositories | [Team graph](../team-graph.md) |
| Audit envelopes and the GitHub Action | [GitHub audit Action](../github-action.md) |
| Personal evidence and agent context | [Persona](persona.md), [context packets](persona-context.md) |

## What it indexes

For each supported file, the index stores these constructs:

| Construct | Python | TS/JS | Rust | C# | Go | Java | PHP | C/C++ |
|---|---|---|---|---|---|---|---|---|
| Functions | ✓ `def` (`.py`/`.pyi`) | ✓ `function` | ✓ `fn` | ✓ method/local-fn | ✓ `func` | ✓ `method_declaration` | ✓ `function` | ✓ definitions + header declarations |
| Module constants | ✓ `FOO = ...` (direct module children only) | n/a | n/a | n/a | n/a | n/a | n/a | n/a |
| Methods | ✓ inside classes | ✓ inside classes | ✓ inside `impl`/`trait` | ✓ inside classes/etc | ✓ `func (r T) M()` | ✓ inside classes | ✓ inside classes | ⚠️ inside classes + `T::m()` def |
| Types | ✓ `class` | ✓ `class`, `interface` | ✓ `struct`, `enum`, `trait` | ✓ `class`, `struct`, `interface`, `record`, `enum` | ✓ `struct`, `interface` | ✓ `class`, `interface`, `enum`, `record` | ✓ `class`, `interface`, `trait`, `enum` | ✓ `class`, `struct`, `union`, `enum` |
| Calls | ✓ | ✓ + `<Component />` JSX | ✓ + `Mod::foo()` | ✓ + `Type.Method()` | ✓ + `pkg.Func()` + `Foo{}` | ✓ + `new Foo()` | ✓ + `Foo::bar()` + `new Foo` | ⚠️ + `new Foo` (no template inst) |
| Decorators/attributes | ✓ `@pytest.fixture` | n/a | ✓ `#[test]` | ✓ `[Fact]` | n/a | ✓ `@Test`, `@GetMapping` | ✓ `#[Test]` (PHP 8) | n/a |
| Imports | ✓ `import`/`from` | ✓ ES module forms | ✓ `use` paths | ✓ `using` | ✓ `import` | ✓ `import` | ✓ `use` | ⚠️ `#include` path resolved to indexed headers for file graphs |
| Macros | n/a | n/a | ✓ `println!` | n/a | n/a | n/a | n/a | ❌ invisible, see [Limitations](#limitations) |
| FQ path | `collections.abc.Iterable` | `'pkg'::default` | `foo::bar::Baz` | `System.Collections::*` | `net/http::*` | `java.util.List` | `App\Foo` | `vector::*` |

Each file also gets a synthetic `<module>` symbol (kind `module`) that owns
module-scope imports and top-level statements.

### Language coverage

| Language | Symbols | Calls | Imports | Known gaps |
|---|---|---|---|---|
| Python | function, method, class, constant (`.py` and `.pyi`) | ✓ direct + `obj.method()` (capital-letter receiver heuristic) | ✓ `import`/`from … import` with aliases and `as` rebinds | Star-import expansion not tracked. Dynamic `getattr` dispatch invisible. `__all__`-filtered re-exports not linked |
| TypeScript | function, arrow-fn, method, class, interface, type-alias | ✓ + `new Foo()` constructors + method calls + `<Component />` JSX usage | ✓ ES named / default / namespace + re-exports | Anonymous default exports lose their name. `export * from` is not expanded to members. JSX component detection uses the uppercase-tag convention |
| JavaScript | function, arrow-fn, method, class | ✓ same walker as TS, including JSX in `.jsx`/`.js` | ✓ ES named / default / namespace | No CommonJS `require()` import edge. Same gaps as TS minus interface and type alias |
| Vue SFC | component (the file), plus every script symbol through the TS/JS walker | ✓ `<BaseButton />` and `<base-button />` template usage, normalized to PascalCase | ✓ the script block's ES imports | Template expressions (`@click="bump"`, `:prop="expr"`) are not parsed. Auto-imported components (`unplugin-vue-components`) have no import edge. `<script>` and `<script setup>` merge into one symbol set |
| Rust | function, method, struct, enum, trait, impl-block, mod, macro-call | ✓ + `Crate::fn()` scoped calls + `macro!` invocations | ✓ `use` paths with aliases and globs | Proc-macros invisible at parse time. `derive` traits stored as decorators, not call edges. Glob `use foo::*` stored as `*` |
| C# | class, struct, record, interface, enum, method, property, namespace | ✓ + `new Foo()` constructors + method calls | ✓ `using` directives + type aliases | Lambdas unnamed. LINQ extension calls not tracked one by one. `partial class` stored per file and collapsed on query by default |
| Go | function, method, struct, interface, type | ✓ + composite literals `Foo{}` + `pkg.Fn()` | ✓ `import` paths with aliases, blank identifier, dot imports | Goroutine launches not marked. Anonymous closures unnamed. Build tags stored as decorators |
| Java | class, interface, enum, record, method, constructor | ✓ + `new Foo()` + method-call expressions | ✓ `import` declarations including static and wildcards | Anonymous inner classes not tracked. Lambda bodies unnamed. Annotation processors invisible |
| PHP | namespace, class, interface, trait, enum, method, function | ✓ + `new Foo`, `Foo::bar()`, `$this->method()` | ✓ `use` with aliases + grouped `use App\{A, B as C}` | Magic methods (`__get`, `__call`) are symbols but their call targets stay unresolved. `call_user_func` targets invisible |
| C/C++ | function and method definitions and declarations, class, struct, union, enum | ⚠️ best effort, no preprocessor and no semantic analysis | ⚠️ `#include` spelling kept and resolved to indexed files for maps and cycles | Macros invisible (`TEST(Suite, Name)` parses as a call). Template instantiations not tracked. Header and source declarations give duplicate rows. ADL and overloads not resolved |

### Path format per language

| Language | Example | Indexed name / path |
|---|---|---|
| Python | `from collections.abc import Iterable as Iter` | `Iter` / `collections.abc.Iterable` |
| TS/JS | `import { foo as bar } from './a'` | `bar` / `./a::foo`. Default: `<src>::default`, namespace: `<src>::*` |
| Rust | `use foo::bar::Baz as Q` | `Q` / `foo::bar::Baz`. Wildcard: `foo::*` |
| C# | `using System.Collections.Generic;` | `Generic` / `System.Collections.Generic::*`. `using X = Y.Z` takes the right side |
| Go | `import f "fmt"` | `f` / `fmt::*`. `_` and `.` aliases use the path leaf |
| Java | `import java.util.List` | `List` / `java.util.List`. Wildcard: `java.util::*`, static imports keep the symbol leaf |
| PHP | `use App\Foo as Bar` | `Bar` / `App\Foo`. Grouped `use App\{A, B as C}` expands per item |
| C/C++ include | `#include <vector>` or `#include "sub/dir/x.h"` | `vector` / `vector::*` or `x.h` / `sub/dir/x.h::*` |
| C++ using | `using std::vector` or `using namespace ns` | `vector` / `std::vector` or `*` / `ns::*` |
| Call target | `obj.foo()` | Literal `obj.foo`, without type resolution |

### Which files get indexed

Manual indexing, automatic refresh and the freshness check all use one source
inventory:

| Input | Rule |
|---|---|
| Inside a Git worktree | `git ls-files --cached --others --exclude-standard`, so Git decides which ignore rules apply |
| Outside Git, or `git` not on `PATH` | A directory walk that applies repository, parent, global and `.git/info/exclude` ignore rules |
| `.ignore` files | Not read |
| Directory the walk may not read, such as a database data folder | Skipped like an ignored directory when the index holds no source under it. When it does, the run fails with `cannot read the directory that holds indexed source PATH`, because skipping it would purge those sources as if they were deleted. Any other walk error fails the run |
| Untracked file with a non-UTF-8 name | Skipped. If its extension is a supported language, the run fails |
| Git path list | Capped at 64 MiB, about a million paths. Past that, indexing fails with `source_inventory_bytes` |
| Tracked file that a new ignore rule matches | Stays an index candidate. Ignore rules still exclude untracked files |
| Extensions | Matched case-insensitively. `.pyi` parses as Python. UTF-16 LE/BE sources with a BOM are decoded first |
| Repository root | Must have an exact UTF-8 canonical path. Any other root is rejected before rows are written |
| Always skipped | `.git`, `.mastermind`, `.venv`, `venv`, `__pycache__`, `node_modules`, `target`, `dist`, `build`, `.tox`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.next`, `.turbo`, `.cache`. The watcher ignores events under them too |
| Git submodules | Not indexed, because their files belong to another repository. `index` prints `skipped submodule:` and `mmcg_status` counts `skipped_submodules`. Index the submodule on its own |
| Unsupported languages | Counted and sampled, not stored |

## CLI usage

Run commands from the repository you inspect. Replace example paths and Git
baselines with your own.

| Command | Use it to | Details |
|---|---|---|
| `init`, `setup`, `uninstall` | Configure or remove a project and its clients | [Project and client setup](#project-and-client-setup) |
| `status`, `doctor` | Check the index, client setup and mining | [Project and client setup](#project-and-client-setup) |
| `index`, `watch`, `query` | Build the index and query symbols | [Index and inspect](#index-and-inspect), [Watcher](#watcher) |
| `enrich`, `facts` | Add SCIP or declarative evidence | [SCIP overlay](#optional-scip-semantic-overlay), [Fact-ingestion SDK](../fact-ingestion-sdk.md) |
| `map`, `impact` | Summarize architecture and change impact | [Maps and change impact](#maps-and-change-impact) |
| `repo-map` | Build a question-specific shortlist | [Question-specific shortlist](#question-specific-shortlist-repo-map) |
| `concept`, `concept-review` | Find symbols by concept and review their relevance | [Concept search](concept-search.md) |
| `brief`, `history`, `why`, `context` | Build role context and search project history | [Project history and role context](#project-history-and-role-context) |
| `temporal` | Compare architecture with a Git baseline | [Temporal graph](#temporal-graph) |
| `team` | Lock and query a graph across repositories | [Team graph](../team-graph.md) |
| `policy` | Check architecture rules | [Policy as code](policy.md) |
| `ui`, `review` | Review a change locally or export evidence | [Lens and review export](lens.md) |
| `workflow` | Audit agent, skill and tool wiring | [Workflow audit](#workflow-audit) |
| `new-spec`, `verify-spec`, `run-task`, `verification`, `acceptance`, `audit-spec` | Run a task through its gates | [Task gates](spec-and-report.md#task-gates) |
| `review-task` | Record the semantic review of a held task | [Semantic task review](spec-and-report.md#semantic-task-review) |
| `next`, `resume` | Print the next action, or the full packet for a pending task, with a ready-to-paste prompt | `mastermind resume --task 042-name` |
| `ci` | Index, verify specs and audit executor evidence in CI | [CI](spec-and-report.md#ci) |
| `audit`, `pr-comment`, `pr-comment-untrusted` | Verify or sign audit envelopes and render them as PR comments | [GitHub audit Action](../github-action.md) |
| `effort-route` | Select effort for a task and agent | [Effort routing](../guides/effort-routing.md) |
| `serve` | Run the MCP stdio server | [MCP server](mcp.md) |
| `miner` | Collect and review personal evidence | [Persona](persona.md) |
| `demo`, `tour` | Try the workflow on a temporary repository, or print a guided walkthrough | `mastermind demo` |

### Index and inspect

```bash
mastermind index .
mastermind index . --force
mastermind watch .
mastermind status
mastermind query search PendingFile
mastermind query outline src/store.rs
mastermind query callers commit_file
mastermind query callers callback --edge-kind references
mastermind query callees process --file src/second.rs --line 12
mastermind query imports src/store.rs
mastermind query imported-by collections.abc.Iterable --match-kind path
mastermind query impact extract --depth 3
mastermind query recent --since 2h
mastermind query unreferenced --kind function
mastermind query api-surface src/runtime/
```

`index` is incremental and `--force` reparses every admitted file. `watch` runs
until you stop it. Unreferenced symbols are candidates to review, and a graph
reference is not proof that the code runs.

Optional evidence goes in with `enrich`:

```bash
mastermind enrich --scip index.scip
mastermind query semantic "scip-clang . my-package . PaymentService#charge()." --top 100
mastermind enrich --facts facts.json
mastermind query facts --path src --top 400
```

### Maps and change impact

```bash
mastermind map . --format text
mastermind map src --format json --depth 2 --top 20
mastermind map . --format mermaid
mastermind map . --format sarif > mastermind-map.sarif
mastermind impact --since main --format text --depth 3 --top 100
mastermind impact --since main --format sarif > mastermind-impact.sarif
mastermind temporal --since HEAD~5 --path services/payment --format json
```

`impact` includes staged, unstaged and untracked changes. Text and JSON keep
file-only changes, graph precision, returned and total counts and the reasons a
result is partial. Map caps and truncation causes are in
[map scope and caps](mcp.md#map-scope-and-caps). SARIF output is described in
[SARIF export](lens.md#sarif-export).

### Question-specific shortlist (`repo-map`)

```bash
mastermind repo-map "payment retry handler" --source-sha256 <verified-source-digest> --budget-bytes 1600
mastermind repo-map "payment retry handler" --source-sha256 <digest> --payload map-only
mastermind repo-map "payment retry handler" --source-sha256 <digest> --ranking graph
```

`repo-map` is experimental. It returns a JSON navigation payload
(`focused_shortlist_v3`) built from weighted task-term overlap: at most three
files and three definitions per file, grouped by file. It leaves spare budget
unused and excludes declared test functions. The result is a query heuristic, so
read the source at the selected locations and follow the call paths yourself.

| Option | Contract |
|---|---|
| `--source-sha256` | SHA-256 of the source projection you validated before the call, together with index freshness and the allowlist. The command does not check it against live files |
| `--budget-bytes` | Byte cap for the returned text. `payload_bytes` charges the text actually returned |
| `--payload bundle\|map-only\|instructions-only` | Component comparison. All variants use the same locations and header reservation. `instructions-only` has no locations. Default `bundle` |
| `--ranking lexical\|graph` | Default `lexical`. `graph` runs personalized PageRank seeded by lexical scores over `calls` and `references` links between definitions with a unique same-language name. Graph links are navigation hints |

| Failure | Result |
|---|---|
| More than 20,000 candidates | The command fails instead of ranking a partial prefix |
| Invalid digest, budget or location, or an index change during the call | Nonzero exit. Rebuild a stale index and compute a new digest |
| Graph overflow (100,000 edges), empty graph or no seeds | Falls back to lexical selection. The `graph` field records the fallback and its counts |
| SQLite query error | The command fails |

The index opens read-only. The [scorecard](../../evals/scorecard.md) lists the
retained repo-map runs.

### Project history and role context

```bash
mastermind history "webhook dedupe"
mastermind history "runtime boundary" --kind audit
mastermind why "why is webhook dedupe durable?"
mastermind brief --role executor --since HEAD --budget-tokens 2000
mastermind context preview --since HEAD --role executor --workflow strict \
  --path src/service.rs --query transport --budget-tokens 8000
```

`history` and `why` return retained evidence with its freshness and coverage,
and they do not fill in rationale that no document records. `brief` builds the
same packet as [`mmcg_brief`](mcp.md#bounded-role-briefs). The context preview
keeps person, project, documentation, code and past work in separate layers,
and personal advice needs an existing grant and an explicit client. See
[context packets](persona-context.md).

### Personal profile and hooks

```bash
mastermind miner profile .
mastermind miner collect --project-root . --transcript /path/to/session.jsonl --dry-run
mastermind miner candidates list --status pending
mastermind miner hooks status --client codex --project-root .
```

Personal profiles are user-global and separate from repository context.
[Persona](persona.md) owns collection, capture, task mining, review and access.
[Persona hooks](../guides/persona-hooks.md) walks through client setup.

### Project and client setup

```bash
mastermind init
mastermind init --client codex --workflow off --mining capture
mastermind status --json
mastermind doctor --json
mastermind workflow audit --root . --json
mastermind setup claude --scope user
mastermind setup cursor --scope project --root . --write
mastermind setup codex --scope user --write
mastermind setup continue --scope project --root . --write
mastermind setup generic --scope project --config ./mcp.json
```

[Getting started](../getting-started.md) owns `init`, `update` and `uninstall`.
Each client guide owns its `setup` scopes, and `setup` previews changes until you
pass `--write`. The native run budgets that `init` stores in
`.mastermind/setup.json` are in [native executor](task-runtime.md#native-executor),
[native semantic reviewer](task-runtime.md#native-semantic-reviewer) and
[initial worktree ownership](task-runtime.md#initial-worktree-ownership).

`doctor` checks configuration, the server handshake, installed agent contracts
and optional hook readiness. It never runs a provider or a configured command.
An optional hook client that is not configured is not a failure.

### Explaining symbol queries

`mastermind query explain <name>` returns the raw exact-name definitions and
diagnostic counts for `calls`. `--language` filters both definitions and
incoming source symbols, and accepts the returned language IDs, including `tsx`
for `.tsx` and `typescript` for `.ts`.

Check `match_status` before you use the outgoing summary:

| `match_status` | `matched` | `callee_count` / `edge_precision` |
|---|---|---|
| `matched` | One raw definition | Its outgoing count and language precision. Zero means no indexed call pairs |
| `ambiguous` | All same-name definitions after the language filter | Both `null`. Use `query callees` with an exact file and declaration start line |
| `not_found` | Empty | Both `null`. No outgoing measurement was made |

| Field | Counts |
|---|---|
| `caller_count` with `caller_count_scope: "name_or_type_candidates"` | Distinct containing source symbols with compatible edges to the name or type prefix. Not tied to one definition, and can be positive without an indexed definition. Repeated call sites in one source count once |
| `callee_count` | Distinct `(target name, call line)` pairs of the one matched definition |
| `edge_precision`, `limitations` | Only that definition's outgoing extraction |
| `precision_notes` | General dependency and count-scope caveats, present in every response |

Partial-class declarations stay separate in `matched`, while `query search` may
group them. Storage and query failures exit with an error. They never produce a
success response with a zero count, and an empty result does not prove there is
no runtime dependency.

## Local symbol concept search

This section moved to [Concept search](concept-search.md).

## Workflow audit

`mastermind workflow audit` reads agent, skill, model, tool and writer wiring
without running any agent instructions. It emits `schema_version: 1` with
`nodes`, `edges`, `diagnostics`, `limits`, `complete` and `context_estimates`.
Node IDs are kind-prefixed (`agent:`, `skill:`, `model:`, `server:`, `tool:`,
`artifact:`, `writer:`). Human and JSON output come from the same report, and
the human form prints every node, edge, estimate, diagnostic and limit.

| Exit | Meaning |
|---|---|
| 0 | Complete input and no error diagnostic |
| 1 | An error diagnostic or incomplete input |
| 2 | Command-line usage error |

`complete` says only that inventory, traversal and input checks finished. A
complete report can still hold error diagnostics, so decide on the exit code and
`diagnostics`.

| Inventory resource | Limit |
|---|---:|
| Agents / skills | 128 / 512 |
| Markdown file / manifest | 256 KiB / 1 MiB |
| Aggregate text | 8 MiB |
| Directory entries / directories / depth | 8,192 / 4,096 / 16 |
| Graph nodes / edges | 4,096 / 16,384 |
| Skill relations per component | 512 |
| Writes / runtime grants / MCP servers per component | 64 / 512 / 64 |
| Admitted writers / diagnostics / context estimates | 512 / 4,096 / 16,384 |

Traversal opens no-follow handles. Admission rejects symlinks, non-regular
files, path escapes, changed read identities, non-UTF-8 Markdown, YAML aliases,
anchors, tags and merge keys, duplicate keys, multiple YAML documents and unknown
workflow metadata. Any skipped input sets `complete: false` and suppresses
findings that need a complete inventory.

When an installed Claude role scopes `mmcg`, the audit reads its registration
only from the project `.mcp.json` beside `.claude` or from the user
`~/.claude.json`. The entry passes when all of these hold:

| Field | Accepted |
|---|---|
| `command` and `args` | The installed binary with `["serve"]`, the project or global `mastermind` launcher, or `npx -y @xcraftmind/mastermind[@version] serve`, in the form for the current platform. Other executables, packages or extra arguments such as `--index` fail |
| `type` | Absent or `stdio` |
| `env` | Absent, empty, or exactly one `MMCG_PROFILE_CLIENT` with a valid client ID, which `setup` writes. Any other variable fails, so it cannot replace the executable or npm behavior |

| Diagnostic codes | Kind |
|---|---|
| `layout_ambiguous`, `mmcg_server_scope_missing`, `mmcg_registration_missing`, `mcp_registration_entry_invalid`, `mmcg_wildcard_grant`, `mmcg_tool_unknown`, `mmcg_prompt_grant_missing`, `model_unsupported`, `effort_invalid`, `max_turns_invalid`, `tool_allowlist_invalid`, `required_skill_missing`, `readonly_mutation_capability`, `artifact_definition_conflict`, `workflow_declaration_limit_exceeded`, `writer_conflict` | Runtime and wiring errors |
| `tool_grant_unreferenced`, `tools_unreachable`, `role_unconditional` | Informational. They do not say a grant is wrong or that a role runs automatically |

Each context estimate is `ceil(UTF-8 bytes / 4)`. Agent bodies, advisory skills
if loaded and known mmcg tool schemas are separate scenarios. Missing optional
skills and built-in tools without a known schema are named. No field is a
guaranteed runtime total.

## Incremental indexing

`mastermind index` compares each file's mtime with the one stored in the index:

| File condition | Index action |
|---|---|
| Untracked and excluded by Git | Not scanned |
| Tracked but now matches an ignore rule | Kept as an index candidate |
| Mtime differs in either direction | Reparsed and committed, counted as `indexed` |
| Mtime matches | Not parsed, counted as `unchanged` |
| Indexed but gone from disk | Removed, counted as `purged` |
| Binary-looking (NUL in the first 8 KiB) or larger than 5 MiB | Skipped, with a count and a bounded path sample. An indexed file that becomes one is removed |
| Unsupported extension | Skipped, with a count and a bounded path sample |
| Not a regular file, such as a symlink | Never indexed, counted as `failed` and printed as `not a regular file`. It does not make the index stale. A source replaced by a symlink is stale only until the next `index` removes its old rows |
| Read error | Counted as `failed`. Its indexed rows stay until it indexes or disappears |

`index` and `ci` exit 1 when any source fails, symlinks included, after they
print the failed paths. The rest of the run is still committed.

```
indexed 3 (unchanged 124, purged 1, skipped binary 2, skipped large 1, failed 0) / scanned 1247 | 87 symbols | 412 edges | 87 concept rows (orphans purged 0, contract rebuilt false) | 4 task specs | 84 history entries (skipped 0, truncated false) | 84 ms
```

The index also stores the extractor and concept-corpus contract versions. When
parser, extractor or concept rules change, the next ordinary `index` run
rebuilds what it needs even if no mtime changed. `status`, `doctor` and
`mmcg_status` report that drift. `status` also checks the durable-history
inventory under the same 30-second deadline as source freshness, and calls the
index up to date only when all four checks pass. `doctor` runs the same scan with
a 10-second deadline and warns when any structural, concept or history input is
stale, incomplete, changing or not checked in time.

Use `--force` when you want a cold rebuild on purpose, when a writer may have
changed content while keeping the exact mtime, or to time a cold index.

The index records the canonical project root on its first run. Reusing it with a
different root is rejected before any write, so orphan cleanup never purges
files because the caller switched scope. Use a separate `--index` path for
another root. `watch` keeps the root it started with.

| Command form | Index path |
|---|---|
| Command with a root | `<root>/.mastermind/mmcg.db` |
| Command without a root | `.mastermind/mmcg.db` in the current directory |
| `serve`, `concept`, `concept-review` | `<repository root>/.mastermind/mmcg.db`, including on Windows. With `serve --root PATH`, the index of that root |
| Override | `--index <path>` or `MMCG_INDEX_PATH` |

An index written by a newer mmcg is refused, never rebuilt. That covers a newer
schema version and a newer extractor or concept contract, and the error names
both versions. Every open checks this, including read-only queries. A schema change from an older index rebuilds the derived
graph and task-search tables in one transaction. Repository identity
and project-history search stay, so `index .` repopulates the
graph in place.

## Optional SCIP semantic overlay

The Tree-sitter graph stays the default, because it is local, incremental and
needs no language toolchain. `mastermind enrich --scip index.scip` reads a
standard SCIP protobuf and atomically replaces only a separate set of
`semantic_*` tables. It never rewrites symbols, edges, files, or history, and a failed import keeps the previous overlay.

| Import limit | Value |
|---|---:|
| Artifact size | 512 MiB |
| Documents | 500,000 |
| Occurrences | 10 million |
| Definitions | 2 million |
| Symbol-information records | 2 million |
| Semantic edges | 5 million |

| Import rule | Behavior |
|---|---|
| Ranges | Typed SCIP ranges and the deprecated packed form |
| Paths | Canonical repository-relative paths only. Paths and symlinks that escape the root are rejected |
| Artifact read | Copied once through a no-follow parent into a private snapshot, then streamed from it. Snapshot and original identity and digest are checked again before the overlay is replaced |
| Sources | Read through one root capability and hashed at import, then checked again by path, identity, size and digest before replacement |
| `project_root` | Must resolve to the indexed repository |
| Moved or portable artifact | Accepted only when every `Document.text` matches its current file. Then sources report `repository_verified` |
| Some document text omitted | Identity is still verified, but the result carries `semantic_artifact_revision_unverified` and stays `partial` |
| Later file changes | Affected facts are suppressed as stale and counted as `omitted_stale`, with `truncated` set. `total` stays exact when the query limit was not reached |
| Unverified identity or unreadable overlay | Collection totals are unknown, never an empty measured result |

SCIP occurrences become `reference` or `import` evidence. Explicit SCIP
relationships stay `implementation`, `type_definition`, `definition` or
`reference`. They are not relabelled as calls, because an occurrence does not
show that a reference is a call.

| Evidence | Provenance |
|---|---|
| Tree-sitter topology | Default and the fallback without SCIP |
| Exact matching SCIP symbol and file endpoint | Preferred static source, `provenance=scip`, `confidence=high` |
| Tree-sitter-only edge | `syntactic`, medium confidence |
| OpenTelemetry | Observed runtime corroboration. It never creates topology |

Inspect definitions and relationships with `mastermind query semantic SYMBOL` or
the [`mmcg_semantic`](mcp.md#compiler-and-external-evidence) tool. Lens loads a
valid overlay automatically and decorates only exact returned endpoint and
display-name pairs. `map`, callers, impact and the other tools work the same
without SCIP.

## Temporal graph

`mastermind temporal --since REF` compares the indexed working copy with a
resolved Git commit without checking out the old tree. It clones the current
SQLite snapshot into a private temporary database, loads the changed blobs at
the baseline, rewinds only changed supported source paths and runs the same
bounded map on both sides. Database and WAL bytes, source files and the Git
index stay unchanged. SQLite may update reader marks in an existing SHM file.
Data-version and database/WAL metadata are checked again, so a running watcher
cannot mix two revisions into one result. A selected scope that was deleted
completely is rebuilt from the baseline.

Schema v1 reports:

| Section | Content |
|---|---|
| Components | Added, removed, and file or language drift |
| Boundaries and `public_api` | Added, removed and signature-drifted cross-component boundaries. This is observed external graph use, not declared visibility |
| Cycles | Introduced, resolved and membership-changed. Growing or merging an existing cycle is not reported as a new one |
| Hotspots | In-degree and rank movement for symbols in both bounded windows, plus entries and exits |
| Ownership | CODEOWNERS changes over the selected source paths, changed files and returned boundary paths, with base and head files read separately under last-match-wins rules. Includes rule-only changes |
| History review candidates | Indexed history that mentions a deleted path, removed component, removed boundary or drifted public API. A correlation, not proof that a decision is obsolete |

| Limit | Value |
|---|---|
| Changed files | 10,000. The change set must be complete before rewind, and an overflow fails |
| Ownership | 500 relevant paths |
| History | 5,000 derived artifacts, 32 MiB, 500 candidates |
| Diagnostics | 100, with `diagnostics_truncated` |

| Coverage state | Result |
|---|---|
| Section or map projection truncated | `partial: true` |
| Source projection complete | Exact `total`, even when only the returned page is bounded |
| Partial base or head map, bounded or missing CODEOWNERS, incomplete history | Affected `total` and summary count are `null` |
| No change observed but a required source incomplete | `summary.architecture_changed` is `null`, not `false` |

Git, CODEOWNERS, history, rewind and SQLite phases all observe the request
budget and cancel signal. The head CODEOWNERS file is a bounded regular-file read
through a no-follow parent, so special files and path swaps fail. A
repository-relative override must be an exact canonical path. Temporal topology
is Tree-sitter evidence only. SCIP, runtime, coverage and test overlays keep
their own provenance in Lens.

The same response comes from [`mmcg_temporal`](mcp.md#architecture-and-change-analysis)
and shows in Lens below the blast trace. Lens isolates an unavailable temporal
result, so impact evidence stays usable, but a repository, Git or SQLite race
still fails the refresh. The default text format keeps the resolved revisions,
returned and total counts with their reasons, every drift category, provenance,
limits and diagnostics.

## Watcher

`mastermind watch` is a long-running process. It subscribes to recursive file
events under the root, builds the initial index while it keeps arriving events,
groups rapid changes per path with a 500 ms debounce, then reindexes changed
files and purges removed ones.

| Event | Behavior |
|---|---|
| Failed update | Stays pending for up to three attempts, one second apart. Recovery may rebuild the index to restore the concept corpus. No new edit is needed for a retry |
| Another writer holds the index | The watcher prints `another process is writing the index, retrying full refresh` and retries a full refresh every second. These retries never use up the three attempts |
| Database, sidecar and writer-lock writes | Not queued |
| Removed path | Purged after the same debounce, only if it is still gone under the writer lock. An editor that saves by replacing the file keeps its rows |
| Source that is not a regular file | Reported on each refresh, not counted as a failed update |
| New deep directory | Files are observed one by one. The new subtree is not rescanned as a whole |
| Event queue or pending paths over 4,096 entries, or retries exhausted | The watcher stops with an error |

After a stop, fix the reported cause, rebuild with `mastermind index ROOT`, then
start the same watch command again. Keep its working directory, root and
`--index` or `MMCG_INDEX_PATH` the same for both commands. Until then the index
may be stale.

MCP structural queries refresh the managed index on their own, so the server
does not need `watch`. Run it when you use CLI queries or Lens, which report a
stale index and never update it. `watch` and `serve` can run against the same
file at the same time, because SQLite WAL mode keeps readers unblocked.

Every index writer (`index`, MCP refresh, `watch` and `enrich`) holds an exclusive lock
file next to the database, such as `.mastermind/mmcg.db-writer.lock`, for the
whole run. A second writer waits up to 30 seconds or its request deadline. Then
`index` fails with `the index is busy` and MCP returns `index_busy`. Readers never
take the lock, and opening a current index writes nothing, so `serve` starts
while an index runs.

Outside a Git worktree, freshness checks and refresh walk the directory with the
same rules as manual indexing. The CLI stale-index warning checks the whole
inventory without the MCP refresh caps, inside the command's
`MMCG_QUERY_BUDGET_MS` budget.

## Environment variables

| Variable | Default | What it does |
|---|---|---|
| `MMCG_INDEX_PATH` | `<root>/.mastermind/mmcg.db` for commands with a root, otherwise `.mastermind/mmcg.db` in the current directory | Index location |
| `MMCG_QUERY_BUDGET_MS` | `10000` for `serve`, `60000` for `query`, `map` and `impact` | Work budget for one MCP tool call or CLI graph query. `0` means unlimited |
| `MMCG_GIT_TIMEOUT_MS` | `30000`, capped at `300000` | Deadline for Git subprocesses in history, diff, verification and audit. A shorter request deadline wins. A stuck process is killed and the operation fails |
| `MMCG_TASK_FINALIZER_WAIT_MS` | `4000`, capped at `120000` | How long the persona task finalizer waits after Claude's Stop hook for the transcript to name the model. After that the binding stays pending until the next local task boundary |
| `MMCG_REQUEST_SOFT_TIMEOUT_MS` | `30000` | `serve` only. The watchdog cancels the request after this time. `0` turns the soft ceiling off |
| `MMCG_REQUEST_HARD_TIMEOUT_MS` | `300000` | `serve` only. The watchdog exits the process after this time. `0` turns the hard ceiling off |
| `MMCG_WATCHDOG` | unset | `0` turns off both watchdog ceilings and the parent check |
| `MMCG_PROFILE_CLIENT` | unset | Client ID the MCP server presents for profile access. `setup` writes it. Without it, or without a matching grant, `mmcg_profile` returns `access_denied`. See [profile access](persona.md#profile-access) |
| `MMCG_TEAM_MANIFEST` | unset | The one team manifest the MCP server may read. Relative paths start at the repository root. See [Team graph](../team-graph.md#use-through-mcp) |
| `MMCG_TEAM_MANIFEST_SHA256` | unset | Expected SHA-256 of that manifest. A mismatch fails the call |
| `MMCG_AUDIT_NARRATIVE` | `.mastermind/audit-narrative.json` | Lens audit narrative path, repository-relative or absolute inside the repository. See [AI audit narrative](lens.md#optional-ai-audit-narrative) |
| `MMCG_INPUT_ORIGIN` | unset | `controller` makes persona capture skip the input. `automation` keeps it but marks it ineligible for personal quotes. `run-task` sets `controller` on its native runs. See [native capture](persona.md#native-capture) |

## Work budgets, timeouts, and cancellation

Every MCP tool call and CLI graph query runs under one absolute deadline and a
cancel state. SQLite, file walks and reads, refresh, Git, retries, snapshots and
serialization all observe it. The serve watchdog is a wall-clock fallback for
work that cannot observe cancellation.

| Operation | Default / maximum | Behavior |
|---|---|---|
| MCP `tools/call` | `MMCG_QUERY_BUDGET_MS=10000`, `0` disables it | Covers the handler, at most one managed refresh and one retry |
| CLI query, map and impact | `MMCG_QUERY_BUDGET_MS=60000` | Same mechanism |
| Change-impact graph walk | 2 s / 250,000 operations | The shorter of the inner and outer budgets wins |
| Git subprocess | 30,000 ms / 300,000 ms | `MMCG_GIT_TIMEOUT_MS`. A shorter request deadline wins and a timeout kills the child |
| Per-file Git diff | 10,000 files | Larger results are prefixes with `truncated: true` |
| Serve soft watchdog | 30,000 ms | `MMCG_REQUEST_SOFT_TIMEOUT_MS` cancels the request |
| Serve hard watchdog | 300,000 ms | `MMCG_REQUEST_HARD_TIMEOUT_MS` exits the server |
| SQLite connection | 5,000 ms busy timeout, 64 MiB cache | `busy_timeout=5000`, `cache_size=-65536` |

When the watchdog exits `serve`, it first removes the server's private index
snapshot from the temporary directory. Windows refuses to delete a file that
SQLite still holds open, so there the snapshot can stay behind, and the server
prints `could not remove index snapshot PATH` on stderr. Delete that directory
by hand once the server has stopped.

An expired MCP budget returns a typed error:

```json
{
  "code": "work_limit_exceeded",
  "budget_ms": 10000,
  "guidance": "narrow scope (subdirectory path, smaller depth, language filter) or raise MMCG_QUERY_BUDGET_MS"
}
```

| Event | Result |
|---|---|
| Change-impact graph budget expires | The walk stops and adds a `graph_work_limit` precision note |
| `notifications/cancelled` or `$/cancelRequest` for the active ID | That request stops and returns `cancelled` |
| Cancel after completion | Does not cancel the next request |
| Another request during a query | Waits, because the serve loop is serial. This includes `ping` |
| Parent process changes | The watchdog exits, so no orphan keeps the inherited MCP stdin |

The npm wrapper uses `stdio: "inherit"`, so the wrapper dying does not always
close the server's stdin. The hard watchdog covers that case.

| Git ref input | Admission |
|---|---|
| Length | Nonempty, at most 1,024 bytes |
| Syntax | No leading `-` and no NUL |
| Resolution | `rev-parse --verify --end-of-options` must return one full lowercase SHA-1 or SHA-256 commit OID |
| Later commands | Use the resolved OID and an explicit `--` path separator |

## Limitations

### Language and extraction coverage

- Other extensions than those in [Language coverage](#language-coverage) are
  skipped and reported in a bounded sample.
- A Vue SFC becomes one component named after the file. Its script uses the
  TypeScript or JavaScript grammar and keeps `.vue` line numbers. Template tags
  emit calls, but template expressions and build-tool auto-imports do not.
- Python module constants are direct module-child assignments only.
  Assignments inside control flow, classes or functions are not constants.
  `mmcg_unreferenced` leaves constants out by default because Python value reads
  are not reference edges. Ask for `kind=constant` explicitly.
- Rust records known function values and parsed macro-body usages as
  `references`, without expanding macros or claiming a call. Macro-body
  reparsing is limited to 64 KiB per body, 1 MiB of reparsed input, 256 parse
  attempts per file and eight nesting levels. Bodies over budget and
  wildcard-imported function values can be missed, and query `truncated` fields
  do not report it.
- C/C++ has no preprocessor, template instantiation, ADL or overload
  resolution. Include resolution tries exact repository-relative,
  source-relative, then suffix matches, so duplicate basenames can
  over-approximate. One `tree-sitter-cpp` grammar parses both C and C++, so C
  identifiers that are C++ keywords can mis-parse. SCIP can add resolved
  evidence separately.

### Symbol and edge identity

- Call targets are name-based candidates. Where syntax allows, the target kind
  is constrained, so a receiver call does not bind a bare free function. Receiver
  type and compiler identity are not resolved. `obj.foo()` keeps `to_name=foo`
  and literal `to_path=obj.foo`.
- Edges store `to_name` and `to_path`, not a resolved cross-file `to_id`. A
  callers query can pool unrelated same-name symbols.
- Import `match: path` compares literal source spelling, so a re-exported
  `crate::Baz` does not match `foo::bar::Baz`. Use `match: name` for broad
  consumers or SCIP for compiler identity.
- TS, JS and Python member calls use the rightmost capitalized receiver as
  `to_type`, so uppercase variable names can give false type hints.
- JSX calls follow the uppercase component convention. Lowercase custom
  components are missed and capitalized host elements in custom renderers are
  counted.
- Variable-declared functions use their binding name. One wrapper layer such as
  `memo(...)`, `forwardRef(...)` or `styled(...)` is unwrapped, deeper wrappers
  can be missed, and a non-function assignment that takes a callback can be
  over-captured.
- C# partial declarations collapse only when name, kind and full namespace
  match. Rows without namespace identity stay separate. `mmcg_callees` selects
  one declaration by `file` and `line`.

Graph precision is at most `medium` for Rust, Go, Java, C# and the other AST
extractors, and `low` for C/C++. `resolution` names the extraction strategy
(`syntactic` or `heuristic`), and `target_resolution: name_based_candidates`
says no compiler or type resolution happened. SCIP evidence keeps its own
provenance and does not upgrade the default queries.

### Heuristic query semantics

- Impact follows both call and reference candidates, so its reach is not runtime
  call reach, and an empty result does not prove there are no dependencies.
  Graph-selected tests have at most `medium` confidence. `high` is only for a
  test symbol that changed directly.
- `disciplines` uses path signals only: frontend extensions, test naming and
  migration paths. Everything else is `unclassified`. A migration signal asks
  for deeper review and does not mean the change is destructive.
- `mmcg_api_surface` returns symbols referenced from outside a prefix, whatever
  their declared visibility.
- `mmcg_recent_changes` reports source mtimes stored in the index. A stale index
  can miss newer changes. Use `git log --since=...` for Git history.
- `mmcg_unreferenced` hides conventional `main` functions and known framework
  entry points: pytest fixtures and marks, common web routes, JIT, task and CLI
  decorators, Rust test attributes, C# test, web and benchmark attributes,
  JUnit and Spring annotations, PHPUnit, Symfony and Livewire attributes, and
  `test_*` functions in test paths. It cannot see undecorated entry points,
  dynamic dispatch, cross-language calls, runtime registries, C++ macro tests or
  Go's `TestXxx` convention. Inspect every result before you delete code.
