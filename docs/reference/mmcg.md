# mmcg CLI and index

`mmcg` builds a local SQLite index of a repository's symbols, calls and imports.
The CLI, the [MCP server](mcp.md) and [Lens](lens.md) all read that index. This
page lists the commands, what gets indexed, freshness, environment variables and
limits. Start with [Getting started](../getting-started.md) if you have not
installed it yet.

`mastermind` and `mmcg` are the same binary. The npm package installs both
names and a Cargo install provides only `mmcg`. Examples use `mastermind`, and
`mastermind COMMAND --help` lists every argument.

## CLI commands

Run commands from the repository you inspect.

| Command | Use it to | Details |
|---|---|---|
| `init`, `setup`, `uninstall`, `status`, `doctor` | Configure, check or remove a project and its clients | [Project and client setup](#project-and-client-setup) |
| `index`, `watch`, `query` | Build the index and query symbols | [Index a repository](#index-a-repository), [Watcher](#watcher) |
| `enrich`, `facts` | Add SCIP or declarative evidence | [SCIP overlay](#optional-scip-semantic-overlay), [Fact-ingestion SDK](../fact-ingestion-sdk.md) |
| `map`, `impact`, `temporal` | Summarize architecture, change impact and drift from a baseline | [Maps and change impact](#maps-and-change-impact) |
| `repo-map` | Build a question-specific shortlist | [Question-specific shortlist](#question-specific-shortlist-repo-map) |
| `concept`, `concept-review` | Find symbols by concept and review their relevance | [Concept search](concept-search.md) |
| `brief`, `history`, `why`, `context` | Build role context and search project history | [Project history](#project-history-and-role-context) |
| `team` | Lock and query a graph across repositories | [Team graph](../team-graph.md) |
| `policy` | Check architecture rules | [Policy as code](policy.md) |
| `ui`, `review` | Review a change locally or export evidence | [Lens and review export](lens.md) |
| `workflow` | Audit agent, skill and tool wiring | [Workflow audit](#workflow-audit) |
| `new-spec`, `verify-spec`, `run-task`, `verification`, `acceptance`, `audit-spec`, `review-task`, `ci` | Run a task through its gates | [Specs, reports and CI](spec-and-report.md) |
| `next`, `resume` | Print the next action, or the full packet for a pending task, with a ready-to-paste prompt | `mastermind resume --task 042-name` |
| `audit`, `pr-comment`, `pr-comment-untrusted` | Verify or sign audit envelopes and render them as PR comments | [GitHub audit Action](../github-action.md) |
| `effort-route` | Select effort for a task and agent | [Effort routing](../guides/effort-routing.md) |
| `serve` | Run the MCP stdio server | [MCP server](mcp.md) |
| `miner` | Collect and review personal evidence | [Persona](persona.md) |
| `demo`, `tour` | Try the workflow on a temporary repository, or print a walkthrough | `mastermind demo` |

## Index a repository

```bash
mastermind index .
mastermind index . --force
mastermind query search PendingFile
mastermind query callers commit_file --edge-kind references
mastermind query callees process --file src/second.rs --line 12
mastermind query imported-by collections.abc.Iterable --match-kind path
mastermind query impact extract --depth 3
```

`index` is incremental. It reparses a file when its mtime differs from the
stored one in either direction, and `--force` reparses every file, for example
after a writer kept the exact mtime while it changed content. The run ends with
one summary line:

```
indexed 3 (unchanged 124, purged 1, skipped binary 2, skipped large 1, failed 0) / scanned 1247 | 87 symbols | 412 edges | 87 concept rows (orphans purged 0, contract rebuilt false) | 40 documentation documents (languages javascript,python,rust,tsx,typescript, secret omitted 0, size omitted 0, unsupported files 3) | 4 task specs | 84 history entries (skipped 0, truncated false) | 84 ms
```

`index` and `ci` exit 1 when any source fails, after they print the failed
paths. The rest of the run is still committed.

### Which files get indexed

Manual indexing, automatic refresh and the freshness check use one source
inventory.

| Input | Rule |
|---|---|
| Inside a Git worktree | `git ls-files --cached --others --exclude-standard`. A tracked file that a new ignore rule matches stays indexed |
| Outside Git, or `git` not on `PATH` | A directory walk with repository, parent, global and `.git/info/exclude` ignore rules. `.ignore` files are not read |
| Unreadable directory | Skipped when the index holds no source under it. Otherwise the run fails with `cannot read the directory that holds indexed source PATH` |
| Git path list | At most 64 MiB, then `source_inventory_bytes` |
| Untracked file with a non-UTF-8 name | Skipped. If it has a supported extension, the run fails |
| Extensions | Case-insensitive. `.pyi` parses as Python. UTF-16 sources with a BOM are decoded |
| Binary (NUL in the first 8 KiB) or larger than 5 MiB | Skipped and counted with up to 20 sample paths |
| Not a regular file, such as a symlink | Counted as `failed` and printed as `not a regular file, never indexes`. It does not make the index stale |
| Read error | Counted as `failed`. Its indexed rows stay |
| Always skipped | `.git`, `.mastermind`, `.venv`, `venv`, `__pycache__`, `node_modules`, `target`, `dist`, `build`, `.tox`, `.pytest_cache`, `.mypy_cache`, `.ruff_cache`, `.next`, `.turbo`, `.cache` |
| Git submodules | Not indexed. `index` prints `skipped submodule:` and `mmcg_status` counts `skipped_submodules` |

### Index location and versions

| Command form | Index path |
|---|---|
| Command with a root | `<root>/.mastermind/mmcg.db` |
| Command without a root | `.mastermind/mmcg.db` in the current directory |
| `serve`, `concept`, `concept-review` | `<repository root>/.mastermind/mmcg.db`, or that of `serve --root PATH` |
| Override | `--index <path>` or `MMCG_INDEX_PATH` |

The index records its canonical project root on the first run and rejects any
other root before a write, so use a separate `--index` for another root. It
also stores its schema, extractor contract (`mmcg-extractors-v10`) and concept
contract (`mmcg-concepts-v5`). After an upgrade changes one, the next `index`
rebuilds what it needs. An index written by a newer mmcg is refused on every
open. `status` checks source and history freshness within 30 seconds, and
`doctor` runs the same scan within 10 seconds.

### Language coverage

Each file also gets a synthetic `<module>` symbol that owns module-scope
imports and statements. Call targets keep their literal spelling, so
`obj.foo()` is stored as `obj.foo`.

| Language | Symbols | Calls | Imports, as name / path | Known gaps |
|---|---|---|---|---|
| Python | function, method, class, module constant | Direct and `obj.method()` | `from collections.abc import Iterable as Iter` is `Iter` / `collections.abc.Iterable` | Star imports, `getattr` dispatch, `__all__` re-exports |
| TypeScript | function, arrow function, method, class, interface, type alias | Calls, `new Foo()`, `<Component />` JSX | `import { foo as bar } from './a'` is `bar` / `./a::foo`. Default `<src>::default`, namespace `<src>::*` | Anonymous default exports lose their name. `export * from` is not expanded |
| JavaScript | function, arrow function, method, class | Same walker as TS | ES imports as in TS | No CommonJS `require()` edge |
| Vue SFC | The file as a component, plus script symbols | `<BaseButton />` and `<base-button />` as PascalCase | The script block's ES imports | Template expressions and auto-imported components |
| Rust | function, method, struct, enum, trait, impl block, mod, macro call | Calls, `Crate::fn()`, `macro!` | `use foo::bar::Baz as Q` is `Q` / `foo::bar::Baz`. Glob `foo::*` | Proc macros. `derive` is a decorator, not a call |
| C# | class, struct, record, interface, enum, method, property, namespace | Calls, `new Foo()` | `using System.Collections.Generic;` is `Generic` / `System.Collections.Generic::*` | Lambdas unnamed. `partial class` stored per file, collapsed on query |
| Go | function, method, struct, interface, type | Calls, `Foo{}`, `pkg.Fn()` | `import f "fmt"` is `f` / `fmt::*` | Goroutine launches, closures |
| Java | class, interface, enum, record, method, constructor | Calls, `new Foo()` | `import java.util.List` is `List` / `java.util.List`. Wildcard `java.util::*` | Anonymous inner classes, lambdas |
| PHP | namespace, class, interface, trait, enum, method, function | Calls, `new Foo`, `Foo::bar()`, `$this->method()` | `use App\Foo as Bar` is `Bar` / `App\Foo`. Groups expand per item | Magic method targets |
| C/C++ | Function and method definitions and declarations, class, struct, union, enum | Best effort, no preprocessor | `#include "sub/dir/x.h"` is `x.h` / `sub/dir/x.h::*`, resolved to indexed files | Macros (`TEST(Suite, Name)` parses as a call), templates, ADL, overloads, duplicate header rows |

### Explain a symbol query

`mastermind query explain <name>` returns the raw exact-name definitions and
`calls` counts, filtered by `--language` such as `tsx` or `typescript`.
`match_status` is `matched` (one definition, with its `callee_count` of distinct
target and line pairs and its `edge_precision`), `ambiguous` (every same-name
definition, counts `null`, use `query callees --file --line`) or `not_found`.
`caller_count` with scope `name_or_type_candidates` counts distinct source
symbols with compatible edges to the name, so it can be positive without an
indexed definition. A storage or query failure exits with an error, never with a
zero count.

## Maps and change impact

```bash
mastermind map src --format json --depth 2 --top 20
mastermind map . --format mermaid
mastermind map . --format sarif > mastermind-map.sarif
mastermind impact --since main --format text --depth 3 --top 100
mastermind temporal --since HEAD~5 --path services/payment --format json
```

`impact` includes staged, unstaged and untracked changes, and keeps file-only
changes, graph precision, returned and total counts and the reasons a result is
partial. Map caps are in [map scope and caps](mcp.md#map-scope-and-caps), and
SARIF rules in [SARIF export](lens.md#sarif-export).

### Question-specific shortlist (`repo-map`)

```bash
mastermind repo-map "payment retry handler" --source-sha256 <digest> --budget-bytes 1600 --ranking graph
```

`repo-map` is experimental. It returns a `focused_shortlist_v3` JSON payload
from weighted task-term overlap: at most three files and three definitions per
file, test functions left out. Read the source at the returned locations before
you rely on it.

| Option or failure | Contract |
|---|---|
| `--source-sha256` | SHA-256 of the source projection you validated. The command does not check it against live files |
| `--budget-bytes` | Byte cap for the returned text, reported back in `payload_bytes` |
| `--payload bundle\|map-only\|instructions-only` | Default `bundle`. `instructions-only` returns no locations |
| `--ranking lexical\|graph` | Default `lexical`. `graph` runs personalized PageRank over `calls` and `references` between uniquely named definitions |
| More than 20,000 candidates, SQLite error, bad input, index change | Nonzero exit |
| More than 100,000 graph edges, empty graph or no seeds | Falls back to lexical, and the `graph` field says why |

### Temporal graph

`mastermind temporal --since REF` and [`mmcg_temporal`](mcp.md#architecture-and-change-analysis)
compare the indexed working copy with a Git commit without a checkout. A
private copy of the index is rewound to the baseline for the changed paths, and
the same bounded map runs on both sides. The index, source files and Git index
stay unchanged.

| Section | Content |
|---|---|
| Components | Added, removed, and file or language drift |
| Boundaries and `public_api` | Added, removed and signature-drifted cross-component boundaries, from observed graph use |
| Cycles | Introduced, resolved and membership-changed |
| Hotspots | In-degree and rank movement, entries and exits |
| Ownership | CODEOWNERS changes over the selected and changed paths, base and head read separately |
| History review candidates | Indexed history that mentions a deleted path, removed component or boundary, or drifted public API |

At most 10,000 changed files (more fails), 500 ownership paths, 5,000 history
artifacts or 32 MiB with 500 candidates, and 100 diagnostics. A truncated
section sets `partial: true`. A partial map, missing CODEOWNERS or incomplete
history makes the affected `total` `null`, and `summary.architecture_changed`
is `null` when a required source is incomplete.

## Project history and role context

```bash
mastermind history "runtime boundary" --kind audit
mastermind why "why is webhook dedupe durable?"
mastermind brief --role executor --since HEAD --budget-tokens 2000
mastermind context preview --since HEAD --role executor --path src/service.rs --query transport
```

`history` and `why` return indexed project records with freshness and coverage,
and never fill in rationale that no document records. The corpus is in
[history search](mcp.md#history-search). `brief` builds the same packet as
[`mmcg_brief`](mcp.md#bounded-role-briefs), and `context preview` is described
in [context packets](persona.md#context-packets).

## Project and client setup

```bash
mastermind init --client codex --workflow off --mining capture
mastermind doctor --json
mastermind setup cursor --scope project --root . --write
```

[Getting started](../getting-started.md) covers `init` and `uninstall`, and
each client guide covers its `setup` scopes. `setup` previews changes until you
pass `--write`. `doctor` checks configuration, the server handshake, installed
agent contracts and optional hooks, and it never runs a provider or a configured
command. The settings `init` stores are in
[project settings](task-runtime.md#project-settings).

### Permission rules

The `permission rules` check in `doctor` reads each client's permission
settings and warns about the ones that let an agent run code it chooses
without asking. It only reads: it never edits a settings file and never prints
rule text, so a token or address inside a rule stays out of the report. Each
finding names the file, the rule's position, a program or key name and why it
is broad.

| Client | Files read | What is read |
|---|---|---|
| Claude Code | `$CLAUDE_CONFIG_DIR/settings.json` (default `~/.claude/settings.json`), `.claude/settings.json`, `.claude/settings.local.json` | `Bash(...)` and `PowerShell(...)` entries in `permissions.allow`, `ask` and `deny`, `permissions.defaultMode` and `skipDangerousModePermissionPrompt`, see [Claude Code permissions](https://code.claude.com/docs/en/permissions) and the [settings reference](https://code.claude.com/docs/en/settings-reference) |
| Codex | `$CODEX_HOME/rules/*.rules` (default `~/.codex/rules`), `.codex/rules/*.rules` | `prefix_rule` calls and their `decision`, see [Codex rules](https://developers.openai.com/codex/rules) |
| Codex | `$CODEX_HOME/config.toml`, `.codex/config.toml`, profile files `$CODEX_HOME/<name>.config.toml` | `approval_policy`, `sandbox_mode` and `default_permissions`, see the [Codex configuration reference](https://developers.openai.com/codex/config-reference) |

A rule is broad when its open part can choose what runs. Quotes do not close
it: `Bash(bash -c "*")` is as open as `Bash(bash -c *)`.

| Reason | Examples that warn | Examples that do not |
|---|---|---|
| Any command | `Bash`, `Bash(*)`, `Bash(* --version)` | |
| Interpreter or shell with no fixed script, or with a code option | `Bash(python:*)`, `Bash(node -e *)`, `Bash(sh -c '*')` | `Bash(python -m pytest *)`, `Bash(node scripts/check.js)` |
| Command that runs another command, unless that command is fixed and known | `Bash(env *)`, `Bash(sudo -u root *)`, `Bash(ssh host *)`, `Bash(xargs -I {} *)`, `Bash(timeout 5 sh *)`, `PowerShell(Invoke-Command *)`, `PowerShell(& *)`, Codex `["find"]` | `Bash(timeout 5 npm test *)`, and `Bash(watch *)` or `Bash(find *)`, which Claude Code refuses to approve itself |
| Package manager or task runner with no fixed subcommand, with one that runs a named command, or with one that installs packages | `Bash(npm *)`, `Bash(npx:*)`, `Bash(npm exec *)`, `Bash(uv run --with *)`, `Bash(docker compose *)`, `Bash(pip install *)`, `Bash(make *)` | `Bash(npm run *)`, `Bash(cargo test *)`, `Bash(make test)` |
| `git` or `gh` with no fixed subcommand, or with one that sets commands | `Bash(git *)`, `Bash(git -c core.sshCommand=x *)`, `Bash(gh alias *)`, Codex `["git"]` | `Bash(git log *)`, `Bash(gh pr view *)` |
| Network fetch with no fixed host, or a fetch piped into a shell | `Bash(curl *)`, `Bash(curl https://*)`, `Bash(curl -o /tmp/x *)`, `Bash(curl -fsSL https://example.com/x.sh \| sh)` | `Bash(curl https://api.example.com/*)` |
| Claude Code skips permission prompts | `permissions.defaultMode` set to `bypassPermissions`, `skipDangerousModePermissionPrompt: true` | other modes |
| Codex runs every command unsandboxed without asking | `approval_policy = "never"` with `sandbox_mode = "danger-full-access"` or `default_permissions = ":danger-full-access"`, in one file or combined | either setting alone |

How the check combines files:

- A broad Claude Code allow rule does not warn when a `deny` or `ask` rule in
  any of the files has the same specifier or covers the whole tool, because
  Claude Code evaluates deny and ask rules from every file first.
- A broad Codex allow rule does not warn when a `prompt` or `forbidden` rule in
  any loaded `.rules` file covers every command it matches, because Codex
  applies the strictest matching decision. Every alternative in a pattern is
  checked.
- For Codex settings, the project file overrides the user file and a profile
  file overrides both, so a user `approval_policy = "never"` and a project
  `sandbox_mode = "danger-full-access"` warn together.
- Claude Code strips wrappers such as `timeout` from a command before matching
  it. Its documentation does not say whether a rule that starts with a
  wrapper is stripped too, so the check reads past the wrapper and judges the
  wrapped command.

The check passes when no file exists or every rule is narrow. It warns for a
broad rule and for a file it cannot read: invalid JSON or TOML, a `.rules` file
outside the subset it reads (for example an alias of `prefix_rule`, lists
nested deeper than eight levels or a pattern with more than 256 combinations),
a symlink, a file over the configuration size limit, or a directory with more
than 64 rules or profile files, where the rest are not read. Unparsed entries
are counted. A rule with a fixed subcommand still runs whatever that
subcommand runs, for example the repository's `package.json` scripts, so a
passing check does not make a rule safe for untrusted code. Managed settings
that an organisation deploys are not read.

## Local symbol concept search

Concept search has its own page: [Concept search](concept-search.md).

## Workflow audit

`mastermind workflow audit --root . --json` reads agent, skill, model, tool and
writer wiring without running any agent instructions. It prints one
`schema_version: 1` report with `nodes`, `edges`, `diagnostics`, `limits`,
`complete` and `context_estimates`. Node IDs are prefixed by kind: `agent:`,
`skill:`, `model:`, `server:`, `tool:`, `artifact:`, `writer:`.

It exits 0 for complete input with no error diagnostic, 1 for an error
diagnostic or incomplete input, and 2 for a usage error. Symlinks, non-regular
files, path escapes, non-UTF-8 Markdown, YAML aliases, anchors, tags, merge
keys, duplicate keys, several YAML documents and unknown workflow metadata are
skipped and set `complete: false`. Each context estimate is
`ceil(UTF-8 bytes / 4)` for one scenario, not a runtime token count.

| Inventory resource | Limit |
|---|---:|
| Agents / skills | 128 / 512 |
| Markdown file / manifest / all text | 256 KiB / 1 MiB / 8 MiB |
| Directory entries / directories / YAML depth | 8,192 / 4,096 / 16 |
| Graph nodes / edges | 4,096 / 16,384 |
| Skill relations, writes, tool grants, MCP servers per component | 512, 64, 512, 64 |
| Writers / diagnostics / context estimates | 512 / 4,096 / 16,384 |

A Claude role that scopes `mmcg` needs its registration in the project
`.mcp.json` beside `.claude` or in `~/.claude.json`. `command` and `args` must
be the installed binary with `["serve"]`, the `mastermind` launcher, or
`npx -y @xcraftmind/mastermind[@version] serve`, with no extra arguments such as
`--index`. `type` is absent or `stdio`, and `env` holds at most
`MMCG_PROFILE_CLIENT`.

| Diagnostic codes | Kind |
|---|---|
| `layout_ambiguous`, `mmcg_server_scope_missing`, `mmcg_registration_missing`, `mcp_registration_entry_invalid`, `mmcg_wildcard_grant`, `mmcg_tool_unknown`, `mmcg_prompt_grant_missing`, `model_unsupported`, `effort_invalid`, `max_turns_invalid`, `tool_allowlist_invalid`, `required_skill_missing`, `readonly_mutation_capability`, `artifact_definition_conflict`, `workflow_declaration_limit_exceeded`, `writer_conflict` | Errors |
| `tool_grant_unreferenced`, `tools_unreachable`, `role_unconditional` | Informational |

## Optional SCIP semantic overlay

```bash
mastermind enrich --scip index.scip
mastermind query semantic "scip-clang . my-package . PaymentService#charge()." --top 100
```

`enrich --scip` reads a standard SCIP protobuf and atomically replaces a
separate set of `semantic_*` tables. The Tree-sitter graph stays as it is, and a
failed import keeps the previous overlay.

| Import limit or rule | Value |
|---|---|
| Artifact size / documents | 512 MiB / 500,000 |
| Occurrences / definitions / symbol information / edges | 10 million / 2 million / 2 million / 5 million |
| Paths | Canonical repository-relative only. Escaping paths and symlinks are rejected |
| `project_root` | Must resolve to the indexed repository |
| Moved artifact | Accepted when every `Document.text` matches its file, then `repository_verified`. With text omitted it is `partial` with `semantic_artifact_revision_unverified` |
| Later file changes | Affected facts are suppressed and counted in `omitted_stale` |

Occurrences become `reference` or `import` evidence, and explicit relationships
stay `implementation`, `type_definition`, `definition` or `reference`, never
calls. An exact SCIP endpoint has `provenance=scip` and `confidence=high`. Lens
decorates exact returned endpoints with it, and `map`, callers and impact ignore
it.

## Watcher

`mastermind watch ROOT` builds the index, then groups file events per path with
a 500 ms debounce and reindexes or purges them. MCP queries refresh the managed
index on their own, so `watch` is for CLI queries and Lens, which never update
the index.

| Event | Behavior |
|---|---|
| Failed update | Retried up to 3 times, 1 second apart |
| Another writer holds the index | Prints `another process is writing the index, retrying full refresh` and retries every second |
| Removed path | Purged after the debounce if it is still gone, so a replace-on-save editor keeps its rows |
| More than 4,096 queued events or pending paths, or retries exhausted | Stops with an error. Fix the cause, run `mastermind index ROOT`, then start the same watch again |

Every writer (`index`, MCP refresh, `watch`, `enrich`) holds
`.mastermind/mmcg.db-writer.lock` for the whole run. A second writer waits up to
30 seconds or its deadline, then `index` fails with `the index is busy` and MCP
returns `index_busy`. Readers never take the lock.

## Environment variables

| Variable | Default | What it does |
|---|---|---|
| `MMCG_INDEX_PATH` | See [index location](#index-location-and-versions) | Index location |
| `MMCG_QUERY_BUDGET_MS` | `10000` for `serve`, `60000` for CLI `query`, `map` and `impact` | Work budget for one MCP call or CLI graph query, at most one refresh and one retry included. `0` means unlimited |
| `MMCG_GIT_TIMEOUT_MS` | `30000`, capped at `300000` | Deadline for Git subprocesses. A shorter request deadline wins |
| `MMCG_TASK_FINALIZER_WAIT_MS` | `4000`, capped at `120000` | How long the persona finalizer waits for Claude's transcript to name the model |
| `MMCG_REQUEST_SOFT_TIMEOUT_MS` | `30000` | `serve` only. The watchdog cancels the request. `0` turns it off |
| `MMCG_REQUEST_HARD_TIMEOUT_MS` | `300000` | `serve` only. The watchdog exits the process. `0` turns it off |
| `MMCG_WATCHDOG` | unset | `0` turns off both watchdog ceilings and the parent check |
| `MMCG_PROFILE_CLIENT` | unset | Client ID for [profile access](persona.md#profile-access). `setup` writes it |
| `MMCG_TEAM_MANIFEST`, `MMCG_TEAM_MANIFEST_SHA256` | unset | The one [team manifest](../team-graph.md#use-through-mcp) the server may read, and its SHA-256 |
| `MMCG_AUDIT_NARRATIVE` | `.mastermind/audit-narrative.json` | [Lens audit narrative](lens.md#optional-ai-audit-narrative) path |
| `MMCG_INPUT_ORIGIN` | unset | `controller` makes persona capture skip the input, `automation` keeps it without personal quotes |

## Work budgets, timeouts, and cancellation

Every MCP call and CLI graph query runs under one deadline and cancel state
that SQLite, file reads, refresh, Git and serialization all observe. An expired
MCP budget returns `work_limit_exceeded` with `budget_ms`. A
`notifications/cancelled` or `$/cancelRequest` for the active ID returns
`cancelled`. The serve loop is serial, so another request, `ping` included,
waits.

| Bound | Value |
|---|---|
| Change-impact graph walk | 2 s and 250,000 operations, then a `graph_work_limit` note |
| Per-file Git diff | 10,000 files, then `truncated: true` |
| SQLite connection | `busy_timeout=5000`, `cache_size=-65536` (64 MiB) |
| Git ref input | 1-1,024 bytes, no leading `-` or NUL, resolved to one full commit OID |

When the parent process changes, the watchdog exits `serve` so no orphan keeps
its stdin. Before it exits, it removes the private index snapshot. Windows can
refuse that delete, and then the server prints
`could not remove index snapshot PATH`. Delete that directory once the server
has stopped.

## Limitations

Graph precision is at most `medium`, and `low` for C/C++.
`target_resolution: name_based_candidates` says no compiler resolution happened,
so use the SCIP overlay for compiler identity.

### Symbol and edge identity

- Edges store the target name and path, not a resolved target, so a callers
  query can pool unrelated same-name symbols.
- Import `match: path` compares literal spelling, so a re-exported `crate::Baz`
  does not match `foo::bar::Baz`.
- TS, JS and Python member calls take the rightmost capitalized receiver as the
  type, so uppercase variable names give false hints. JSX follows the uppercase
  component convention.
- Python module constants are direct module-child assignments only, and
  `mmcg_unreferenced` lists them only for `kind=constant`.
- Rust macro bodies are reparsed up to 64 KiB per body, 1 MiB and 256 parses
  per file and 8 nesting levels, without a `truncated` flag.
- C/C++ include resolution falls back to suffix matches, so duplicate basenames
  can over-approximate. C identifiers that are C++ keywords can mis-parse.

### Heuristic query semantics

- Impact follows call and reference candidates, so an empty result does not
  prove there are no dependencies. Graph-selected tests have at most `medium`
  confidence.
- `disciplines` uses path signals only: frontend extensions, test naming and
  migration paths. Everything else is `unclassified`.
- `mmcg_recent_changes` reports stored mtimes, not Git history.
- `mmcg_unreferenced` hides `main`, known framework entry points, test
  attributes and `test_*` functions in test paths. It cannot see undecorated
  entry points, dynamic dispatch, runtime registries, C++ macro tests or Go's
  `TestXxx` convention. Inspect every result before you delete code.
