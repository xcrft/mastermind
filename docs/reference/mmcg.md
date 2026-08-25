# mmcg — Mastermind Codegraph

Need the exact command, limit, schema, precision caveat, or MCP shape? This is
the source of truth for the `mmcg` engine. Start with
[Getting started](../getting-started.md) for installation or the
[project README](../../README.md) for the product overview.

`mmcg` is a Rust binary that builds a local structural index for Python,
TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust, C#, Go, Java, PHP, and C/C++.
It exposes the same indexed state through CLI, Lens, and MCP. The MCP surface
contains 28 tools: 19 non-destructive queries that may refresh the managed
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
- MCP exposes 28 bounded tools: 19 non-destructive queries that may refresh the
  managed derived index, 8 read-only queries, and the additive, gitignored
  `mmcg_scratchpad_append` write.

## Performance model

- **Parsers**: tree-sitter (C, vendored — no system tree-sitter required)
- **Parallelism**: `rayon` parses files in parallel; writes serialize through a single SQLite connection (WAL mode)
- **Bounded batching**: at most 64 parsed files are retained before the single SQLite writer commits them; each file uses one transaction via `Store::commit_file`
- **Source admission**: source-looking files above 5 MiB or containing a NUL byte in the first 8 KiB are skipped before parsing; UTF-16 BOMs are admitted and decoded
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
# evidence envelope and never invents rationale absent from the records.
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

- **untracked and ignored by Git or `.ignore` rules** → do not scan; tracked files remain candidates even when a broad ignore rule matches them
- **mtime newer than stored** → re-parse and commit (counted as `indexed`)
- **mtime equals stored** → skip without parsing (counted as `unchanged`)
- **file in index but not on disk** → purge from index (counted as `purged`)
- **binary-looking or larger than 5 MiB** → skip safely and report the count plus a bounded path sample
- **unsupported extension** → skip and report the count plus a bounded path sample

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
streams top-level protobuf fields in bounded passes instead of retaining the
full SCIP index and all embedded source text in memory. Source files are hashed at
import. The reported `project_root` must resolve to the indexed repository; a
portable or moved artifact is accepted only when every `Document.text` exactly
matches its current file. Successful sources expose `repository_verified`.
When some text is omitted, repository identity is still verified but the result
carries `semantic_artifact_revision_unverified`. Later file changes suppress
affected semantic facts as stale rather than silently mixing revisions.

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
`team-manifest` claims and are never inferred.

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
The repository index, source files, Git index, and source WAL/SHM files remain
unchanged. Data-version plus source database/WAL metadata are rechecked so a
concurrent watcher cannot mix two SQLite revisions into one result. A fully
deleted selected scope is reconstructed from the baseline instead of failing
as an empty head map.

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
truncated. Component, boundary, cycle, and hotspot claims therefore describe
the returned deterministic window when the underlying map is partial. Ownership
checks at most 500 relevant paths; history scans at most 5,000 derived artifacts
and 32 MiB and returns at most 500 candidates. Diagnostics are capped at 100
with an explicit `diagnostics_truncated` flag. Git, CODEOWNERS, history, rewind,
and SQLite phases cooperatively observe the request budget/cancel signal.
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
bounded collection in memory.

The server binds only to `127.0.0.1`; port `0` is the default and lets the OS
choose a free port. It accepts same-origin `GET`/`HEAD` requests, serves embedded
offline assets under a restrictive content-security policy, and opens the
existing SQLite index in query-only mode. A checkpointed index is opened
directly as immutable; an active WAL is copied with the database into a bounded
private temporary snapshot (2 GiB and at most 60 seconds, or the shorter request
deadline), so Lens never creates or changes source sidecars.
Refreshes fail closed when the repository, index, WAL, baseline, or work
snapshot changes, or when indexed source files disappeared. There are no
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
`MMCG_AUDIT_NARRATIVE`. Start from `audit.narrative_binding` in the current
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
  This is enabled by default; `--no-project-knowledge` disables it;
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
the derived SQLite history corpus; Markdown remains authoritative and must be
re-indexed after changes. Lens writes none of this evidence to source files or
SQLite.

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
`--codeowners`, `--git-commits`, and `--no-project-knowledge` inputs as Lens.
It rechecks external files after analysis and fails if their exact bytes change.
The manifest records their SHA-256 digests next to the resolved head OID. This
is a **digest binding at export time**: it proves exactly which report bytes a
reviewer saw at that revision, not that Semgrep, CodeQL, a test runner, or an
OTel collector produced those bytes from that revision.

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
or embedded general-purpose runtime.

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

The default workflow evidence directory is `.mastermind/tasks`; CI must restore
the canonical task artifacts or point `--workflow-evidence PATH` at the
downloaded evidence directory. CODEOWNERS is discovered in GitHub priority
order, or overridden with `--codeowners PATH`.

Policy topology stays on the fast default syntactic graph in v1. Imported SCIP
and runtime overlays retain their provenance for Lens but do not add or remove
policy edges. Strict workflow evidence validates canonical local artifact
consistency; it does not prove an audit signature, GitHub approval, or remote CI
execution.

Impact defaults are depth 3 and top 500 for policy checks. Override them with
`--depth 1..5` and `--top 1..500`. Import/cycle work is capped at 50,000 file
edges; baseline cycle comparison is capped at 500 files and 32 MiB; workflow
evidence is capped at 1,000 tasks and 1 MiB per artifact. A strict snapshot can
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
identity before any refresh. A custom external `--index` remains query-compatible
when fresh but requires an explicit `mmcg index` when stale. Failed or unavailable
refreshes return the structured `index_stale` result.

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
| `mmcg_semantic` | `symbol`, optional `top` (default 100, max 500) | Compiler-resolved SCIP definitions, references, implementations, type definitions, and explicit provenance. Returns `fallback_active: true` instead of an error when no overlay exists. Stale document facts are omitted with diagnostics. |
| `mmcg_facts` | optional `path` (default `.`), `top` (default 100, max 400) | Normalized `mastermind-facts/v1` annotations and relationships plus capability negotiation, exact repository/revision identity, provenance, source state, limits, and stale/truncation diagnostics. Read-only; it never loads producer code or creates graph topology. |
| `mmcg_team_map` | `manifest` | Bounded `mastermind-team/v1` graph over pinned local read-only indexes. The locked manifest must be repository-relative, inside the MCP server root, and exactly authorized by `MMCG_TEAM_MANIFEST` plus `MMCG_TEAM_MANIFEST_SHA256`. Nodes are repository-namespaced; internal imports retain Tree-sitter provenance and cross-repository edges are explicit manifest claims. |
| `mmcg_map` | optional `path` (default `.`), `depth` (1–6, default 2), `top` (1–100, default 20), `production_only` (default `false`) | Schema-v1 architecture briefing with lexical file/directory scope: `%` and `_` are literal bytes, selected-directory components are relative to that directory, root components remain repository-relative, and selected files retain their paths. `production_only` excludes conventional test/fixture/example/generated/vendor path segments and test filenames (`test_*`, `*_test.*`, `*.test.*`, `*.spec.*`, `*Test.*`, `*Tests.*`) before bounded queries run. Hotspots prefer unambiguous definitions before pooled same-name collisions. JSON, text, Mermaid, and CLI SARIF are projections of the same result; Mermaid includes component counts/languages, boundaries, hotspots, and cycle rings, while SARIF exports returned cycles as architecture findings. Caps are 50,000 aggregation paths, 20 languages, 20 components, 20 boundaries/component and 400 globally, 50 entry points, 100 hotspots, 50,000 scoped cycle edges, 50 cycles, and 500 cycle memberships. `path_work_limit` marks path-derived partial aggregates; `top_probe` marks a hotspot or per-component boundary cap+1 probe; `global_probe_limit` marks components whose certainty was prevented by the 401st global boundary row; cycle `work_limit` returns no cycles because SCC analysis was skipped before truncated edges could be analyzed. |
| `mmcg_temporal` | `since`, optional `root`, `path` (default `.`), `depth` (1–5, default 2), `top` (1–100, default 20), `production_only`, `codeowners` | Schema-v1 base-vs-indexed-worktree architecture delta. It rewinds changed Git blobs only in a private SQLite snapshot and reports components, public boundaries/API, cycles, centrality/hotspot drift, base/head CODEOWNERS changes, exact history review candidates, provenance, limits, and partial diagnostics. A truncated 10,000-file change set fails closed. |
| `mmcg_change_impact` | `since`, optional `root`, `depth` (1–5), `top` (1–500) | Stable schema-v1 analysis of the resolved baseline against staged, unstaged, and untracked content. Reports added/removed/signature/body-changed symbols, batched transitive callers, component crossings, ranked test candidates, a `disciplines` block routing the change to an evidence set, exact collection metadata, caps, and precision notes. Root, SHA-256 index freshness, Git snapshot, and SQLite snapshot checks fail closed with stable codes. |
| `mmcg_test_impact` | `since`, optional `root`, `depth` (1–5), `top` (1–500) | Exact test-focused projection of `mmcg_change_impact`. Changed tests and depth-1 graph tests are direct, deeper graph tests are transitive, and scoped filename candidates are heuristic. Focused candidates never replace the repository's full required gate. |
| `mmcg_tasks` | `query`, optional `top` (default 10) | Full-text search past task specs (`.mastermind/tasks/<NNN>-<name>/spec.md`). FTS5 MATCH syntax (bare words AND-joined, `"phrases"`, `OR`/`NOT`). Returns paths, titles, and snippet excerpts with `«match»` highlights ranked by BM25. Use as planner pre-flight: "have we touched this area before?" surfaces past designs and prior verdicts. Top-level files prefixed with `_` (e.g. `_lessons.md`) and bare `.md` files at the top of `tasks/` (legacy 0.6.x layout) are intentionally excluded. |
| `mmcg_history` | `query`, optional `kind`, `top` (default 10) | Searches `CONTEXT.md`, `CONTEXT-archive-*.md`, canonical task specs, executor reports, audits, `.mastermind/releases/*.md`, legacy task-local release notes, lessons, and Markdown architecture decisions under conventional ADR directories. `architecture_decision` is an exact `kind` filter. `candidate` lessons are unresolved signals, not active guidance. Returns observed matches, `skipped_artifacts`, `truncated`, and an explicit retrieval-only epistemic contract. Markdown remains authoritative; re-index after Markdown changes. Each artifact is capped at 1 MiB and the corpus at 5,000 files. |
| `mmcg_dependency_cycles` | optional `language`, `min_size` (default 2) | Detect circular imports — strongly-connected components in the file-level import graph (Tarjan's algorithm). Each result is a cycle = a list of files. Pre-merge guard ("does this PR introduce a new cycle?") and architectural-hygiene survey. Resolves edges by leaf-name match — over-approximates (two unrelated `Logger` symbols cross-link) so verify before refactoring. Bump `min_size` to hide trivial A↔B and surface only larger structural problems. Work-capped at 50,000 file-pair edges: above that, `truncated: true` with an empty `cycles` list — incomplete and possibly inaccurate, not "more available"; narrow with `language` and retry. |
| `mmcg_symbols_changed_since` | `git_ref`, optional `root` | Symbol-level diff between a git ref and the current index. Returns `{added, removed, signature_changed}` symbol sets for files in `git diff --name-only <ref>..HEAD`. Re-parses old blobs from `git show <ref>:<path>` using the same extractor. Different from `mmcg_recent_changes` (watcher mtime) — this is git-ref-based, answering "what symbols did THIS PR/branch touch?". PR-review pre-flight, auditor verification, "what new public API appeared in v2.3?". Git subprocesses are killed after `MMCG_GIT_TIMEOUT_MS`; the per-file loop is capped at 10,000 files with `truncated: true` marking a partial diff. |
| `mmcg_status` | — | Index path, file/symbol counts, and bounded `stale_files`. A non-zero value means the next structural query will refresh a managed index, or that a custom external index needs an explicit `mmcg index`. |

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
| `MMCG_QUERY_BUDGET_MS` | no | `10000` for `mmcg serve`; `60000` for `mmcg query`/`mmcg map`/`mmcg impact` | Wall-clock work budget for a single MCP tool call or CLI graph query. `0` = unlimited. See [Work budgets, timeouts, and cancellation](#work-budgets-timeouts-and-cancellation). |
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
  excluded from `mmcg_unreferenced` by default because value reads are not a
  general edge type; request `kind=constant` explicitly.
- C/C++ parsing has no preprocessor, template instantiation, ADL, or overload
  resolution. Macros such as `TEST(Suite, Name)` are calls, not definitions.
  Header declarations and source definitions remain separate rows. Include
  resolution tries exact repository-relative, source-relative, then
  deterministic suffix matches and may over-approximate duplicate basenames.
  One `tree-sitter-cpp` grammar handles C and C++; unusual C identifiers that
  are C++ keywords can mis-parse. SCIP can add resolved evidence separately.

### Symbol and edge identity

- Calls resolve by name, not receiver type. `obj.foo()` records `to_name=foo`
  and literal `to_path=obj.foo`; it does not establish the type of `obj`.
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
  Callers, callees, impact, and outline remain name-based.

### Heuristic query semantics

- `disciplines` derives only path-level signals. Frontend extensions, test
  naming, and migration paths can trigger a discipline; everything else is
  `unclassified`. A migration signal indicates review depth, not destructive
  behavior.
- `mmcg_api_surface` is empirical: it returns symbols currently referenced from
  outside a prefix, regardless of declared visibility. It is not a language
  public-API declaration query.
- `mmcg_recent_changes` reports index timestamps, not Git history. After a
  rebase or forced re-index, use `git log --since=...` for Git truth.
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
