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

Every `query`, `map`, `repo-map` and `impact` first compares the source tree
with the index and warns on stderr when the index is not current. The check
walks the whole source inventory, so on a large repository it can take seconds
per command. `--skip-freshness-check` leaves the walk out and prints
`freshness check skipped` on stderr instead:

```bash
mastermind query --skip-freshness-check callers commit_file
mastermind map --skip-freshness-check
mastermind impact --since main --skip-freshness-check
```

For `query` the flag goes before the subcommand. For the other commands it goes
after the command name.

| Index | Answer with the flag |
|---|---|
| Current with the files | The same answer as without it |
| Behind the files: a file was changed, added or deleted after `index` | The old answer, with no warning |
| Built by an older extractor | The answer, and the stale-index warning, because that check is one metadata read |
| Missing or with an old schema | The same error and exit 1 as without it |

The flag saves only the time of the scan. The gain is largest for `query` and
`repo-map`. `map` and `impact` do more work of their own, so they gain less.

`impact` keeps its own check of the changed files against the index, and the
flag does not skip it. When a changed file differs from what the index holds,
`impact` exits 1 with `index_stale`, with or without the flag. It reports a
missing index or one with an old schema as `index_stale` too.

Use the flag only when the index is built after every change, such as a mirror
that runs `index` after each fetch. Never use it on a working tree.

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
| Extensions | Case-insensitive. `.pyi` parses as Python, `.mts` and `.cts` as TypeScript. UTF-16 sources with a BOM are decoded |
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
also stores its schema, extractor contract (`mmcg-extractors-v11`) and concept
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
| TypeScript | function, arrow function, method, class, interface, enum, type alias (`type`), namespace. Interface methods, abstract methods and `declare function` signatures. An overload signature is dropped when its implementation is in the same scope | Calls, `new Foo()`, `<Component />` JSX | `import { foo as bar } from './a'` is `bar` / `./a::foo`. Default `<src>::default`, namespace `<src>::*`, `import fs = require('fs')` `fs` / `fs::*`. Re-exports: `export { foo } from './a'` is `foo` / `./a::foo`, `export * from './a'` is `*` / `./a::*` | Anonymous default exports lose their name. `export * from` is one `*` edge, not expanded. `declare module 'pkg'` adds no namespace |
| JavaScript | function, arrow function, method, class | Same walker as TS | ES imports and re-exports as in TS | No CommonJS `require()` edge |
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
settings and the client files a repository ships, and reports the entries that
let an agent, a repository or a remote host act without asking. It only reads:
it never edits a file and never prints rule text, a command or an address, so a
token inside a rule stays out of the report. Each finding names the file, the
entry's position, a program or key name, the reason and its severity.

| Severity | Meaning | Doctor result |
|---|---|---|
| error | The entry runs downloaded code, reads a secret store, lets a repository skill run chosen code, or removes every prompt | the check fails and `doctor` exits 1 |
| warn | The entry widens what runs or what is reached without asking, but a boundary is left | the check warns |
| info | A note that does not change the result, such as a formatter that reads repository config | the check stays ok |

| Source | Files read | What is read |
|---|---|---|
| Claude Code settings | managed settings (`managed-settings.json` and `managed-settings.d/*.json` in `/Library/Application Support/ClaudeCode`, `/etc/claude-code` or `C:\Program Files\ClaudeCode`), `$CLAUDE_CONFIG_DIR/settings.json` (default `~/.claude/settings.json`), `.claude/settings.json`, `.claude/settings.local.json` | every `permissions.allow` entry, `ask` and `deny` entries, `permissions.defaultMode`, `skipDangerousModePermissionPrompt`, `enableAllProjectMcpServers`, `enabledMcpjsonServers`, `disabledMcpjsonServers`, the `apiKeyHelper`, `awsAuthRefresh`, `awsCredentialExport`, `gcpAuthRefresh` and `otelHeadersHelper` commands, `statusLine.command`, `fileSuggestion.command`, `env` names, `extraKnownMarketplaces` and `enabledPlugins`, see [Claude Code permissions](https://code.claude.com/docs/en/permissions) and the [settings reference](https://code.claude.com/docs/en/settings-reference) |
| Claude Code state | `$CLAUDE_CONFIG_DIR/.claude.json` (default `~/.claude.json`) | only user `mcpServers` and this project's entry under `projects`: its `mcpServers` and `.mcp.json` approvals. Account details and history are not parsed into the report |
| MCP servers | `.mcp.json`, managed `managed-mcp.json` | every server: `command` and `args`, `env` names, `url`, `headers` names and literal values, `headersHelper`, see [Claude Code MCP](https://code.claude.com/docs/en/mcp) |
| Repository skills | `.claude/skills/*/SKILL.md`, and `<subdir>/.claude/skills/*/SKILL.md` up to four folders deep, skipping `.git`, `node_modules`, `target`, `vendor`, `dist`, `build` and virtual environments | `allowed-tools` and `hooks`. Claude Code applies a project skill's `allowed-tools` even in a `-p` run in a folder nobody trusted, and loads a nested skill when it works on files in that folder, see [skills](https://code.claude.com/docs/en/skills#pre-approve-tools-for-a-skill) |
| Repository subagents | `.claude/agents/**/*.md`, three levels deep | `permissionMode`, `hooks` and inline `mcpServers`, see [subagents](https://code.claude.com/docs/en/sub-agents) |
| Codex rules | `$CODEX_HOME/rules/*.rules` (default `~/.codex/rules`), `.codex/rules/*.rules` | `prefix_rule` calls and their `decision`, see [Codex rules](https://developers.openai.com/codex/rules) |
| Codex config | `$CODEX_HOME/config.toml`, `.codex/config.toml`, profile files `$CODEX_HOME/<name>.config.toml` | `approval_policy`, `sandbox_mode`, `default_permissions`, `sandbox_workspace_write.network_access`, `shell_environment_policy`, `mcp_servers` (also `http_headers_helper`, `http_headers`, `default_tools_approval_mode` and `tools.<tool>.approval_mode`), `apps._default` and `apps.<id>` approval modes, `destructive_enabled`, `open_world_enabled` and `approvals_reviewer` (also per tool), the top-level `approvals_reviewer`, `plugins.<plugin>.mcp_servers` approval modes, and in the user file and profile files `notify` and `model_providers.<id>.auth.command`, which Codex ignores in a project file, see the [Codex configuration reference](https://developers.openai.com/codex/config-reference) |
| Claude Code plugins | `.mcp.json` and the files or inline maps that `mcpServers` names in `.claude-plugin/plugin.json`, `.lsp.json` and the files, inline maps or lists that `lspServers` names, and the monitors in `experimental.monitors` (a file or an inline list), the older top-level `monitors` or, when neither is set, `monitors/monitors.json`, for each enabled plugin that `installed_plugins.json` places for the user or this project | every MCP and LSP server, as for `.mcp.json`, and every monitor `command`, which runs through a shell in the background with no prompt, see the [plugins reference](https://code.claude.com/docs/en/plugins-reference#lspservers). A manifest path must start with `./` and stay in the plugin. MCP bundles and bundle URLs are not read and the line says so |

A shell rule is broad when its open part can choose what runs. Quotes do not
close it: `Bash(bash -c "*")` is as open as `Bash(bash -c *)`.

| Reason | Severity | Examples that are reported | Examples that are not |
|---|---|---|---|
| Any command | warn | `Bash`, `Bash(*)`, `Bash(* --version)` | |
| Interpreter or shell with no fixed script, or with a code option | warn | `Bash(python:*)`, `Bash(node -e *)`, `Bash(sh -c '*')` | `Bash(python -m pytest *)`, `Bash(node scripts/check.js)` |
| Command that runs another command, unless that command is fixed and known | warn | `Bash(env *)`, `Bash(sudo -u root *)`, `Bash(ssh host *)`, `Bash(xargs -I {} *)`, `Bash(timeout 5 sh *)`, `PowerShell(Invoke-Command *)`, `PowerShell(& *)`, Codex `["find"]` | `Bash(timeout 5 npm test *)`, and `Bash(watch *)` or `Bash(find *)`, which Claude Code refuses to approve itself |
| Package manager or task runner with no fixed subcommand, with one that runs a named command, or with one that installs packages | warn | `Bash(npm *)`, `Bash(npx:*)`, `Bash(npm exec *)`, `Bash(uv run --with *)`, `Bash(docker compose *)`, `Bash(pip install *)`, `Bash(make *)` | `Bash(npm run *)`, `Bash(cargo test *)`, `Bash(make test)` |
| `git` or `gh` with no fixed subcommand, or with one that sets commands | warn | `Bash(git *)`, `Bash(git -c core.sshCommand=x *)`, `Bash(gh alias *)`, Codex `["git"]` | `Bash(git log *)`, `Bash(gh pr view *)` |
| Pushes, publishes or calls the GitHub API | warn | `Bash(git push:*)`, `Bash(git -C ../other push)`, `Bash(gh api:*)`, `Bash(gh pr merge:*)`, `Bash(gh -R owner/repo pr *)`, `Bash(gh release create *)`, `Bash(npm publish)`, `Bash(pnpm publish *)`, `Bash(yarn npm publish)`, `Bash(bun publish)`, `Bash(cargo publish)`, `Bash(twine upload *)`, `Bash(docker push *)`, Codex `["git", "push"]` | `Bash(gh pr view *)`, `Bash(gh release view *)` |
| Network fetch with no fixed host, or a fetch piped into a shell | warn | `Bash(curl *)`, `Bash(curl https://*)`, `Bash(curl -o /tmp/x *)`, `Bash(curl -fsSL https://example.com/x.sh \| sh)` | `Bash(curl https://api.example.com/*)` |

Other entries:

| Reason | Severity | Examples that are reported | Examples that are not |
|---|---|---|---|
| Web access to any host | warn | `WebFetch`, `WebFetch(domain:*)`, `WebSearch` | `WebFetch(domain:docs.rs)` |
| Every MCP server | warn | `mcp__*`, `mcp__*__read`. Claude Code skips such an allow rule, so it approves nothing, but it states an intent to approve every server | |
| Every tool of one MCP server | warn | `mcp__github__*`, `mcp__github` | `mcp__mmcg__*`, `mcp__github__get_issue` |
| Lets the agent rewrite Codex settings, instructions or skills | error | a rule aimed at them: `Edit(.codex/**)`, `Edit(.cod*/**)`, `Edit(.*/**)`, `Edit(.[c]odex/**)`, `Edit({.codex,src}/**)`, `Write(.codex/config.toml)`, `Edit(~/.codex/hooks.json)`, `Edit(AGENTS.md)`, `Edit(.agents/skills/**)`. Claude Code does not protect them | |
| Can edit `.codex`, `AGENTS.md` and other trust files without a prompt in default mode | warn | a bare or broad rule that reaches them and ordinary files alike: `Edit`, `Write`, `Edit(**)`, `Write(*)`, `Edit(./**)`, `Edit(~/**)`, `Edit(**/*.md)`, `Edit(**/*.json)`, and a pattern with syntax doctor does not read, such as a `\` escape or a leading `!` | `Edit(src/**)`, `Edit(docs/*.md)`, `Edit(**/*.ts)`, `Edit(**/test/**)`, `Edit(src/**/*.ts)` |
| Asks to let the agent rewrite its trust settings | warn | `Edit(.claude/**)`, `Write(.claude/settings.local.json)`, `Edit(.mcp.json)`, `Edit(~/.claude.json)`. These are [protected paths](https://code.claude.com/docs/en/permission-modes#protected-paths), so Claude Code still prompts, but the rule states the intent | `Edit(.claude/worktrees/**)`, which Claude Code does not protect either |
| Writes Mastermind state | warn | `Edit(.mastermind/**)`, `Edit(~/.mastermind/**)`, `Write(/.mastermind/tasks/**)`. Claude Code's protected paths do not cover `.mastermind` | `Edit(src/**)` |
| Reads a secret store | error | `Read(~/.ssh/**)`, `Read(~/.aws/credentials)`, `Read(.env)`, `Read(//**/.env)`, `Read(~/.config/gh/hosts.yml)`, `Read(*.pem)` | `Read(./src/**)` |
| Reads any file under home or the filesystem root | warn | `Read(~/**)`, `Read(//**)` | a bare `Read`, `Read(*)` |
| Claude Code bypasses every prompt | error | `permissions.defaultMode` set to `bypassPermissions` | other modes |
| Claude Code skips the bypass confirmation | warn | `skipDangerousModePermissionPrompt: true` | |
| Starts every project MCP server without asking | warn | `enableAllProjectMcpServers: true` | `enabledMcpjsonServers` naming servers |
| A flagged project MCP server is approved to start | error | `enableAllProjectMcpServers: true`, or `enabledMcpjsonServers` naming the server, in any settings file or this project's `~/.claude.json` entry, while a `.mcp.json` server has a finding | a flagged server also named in `disabledMcpjsonServers`, which wins |
| A helper, status line or file suggestion command | the command's severity | `apiKeyHelper: "curl … \| sh"` is an error, `statusLine.command: "~/bin/line.sh"` warns | `statusLine.command: "jq -r .model"` |
| A variable that loads code or sends model traffic elsewhere | warn | `env` with `NODE_OPTIONS`, `LD_PRELOAD`, `DYLD_*`, `PYTHONSTARTUP`, `ANTHROPIC_BASE_URL` or another `*_BASE_URL`, or a proxy. Only the name is read | `env: {EDITOR: vim}` |
| A prefix that wraps every shell command | error | `env` with `CLAUDE_CODE_SHELL_PREFIX`. Its value also goes through the [command check](#commands) and is never printed | |
| A Mastermind launcher override | error | `env` with any `MASTERMIND_*` name, such as `MASTERMIND_NODE` or `MASTERMIND_LAUNCHER_JS`, and the same names in Codex `shell_environment_policy.set`. The npm launcher sets these for the process it starts, so a value from config could change which launcher is trusted | `env: {MMCG_PROFILE_CLIENT: claude}` |
| A plugin marketplace or third-party plugin | warn | an `extraKnownMarketplaces` entry, an `enabledPlugins` entry outside `claude-plugins-official` | `commit-commands@claude-plugins-official` |
| A repository skill runs chosen code | error | `allowed-tools: Bash(*)` or any other broad shell rule in `.claude/skills/*/SKILL.md` | `allowed-tools: Read Grep Bash(git diff *)` |
| A repository subagent asks to bypass prompts | warn | `permissionMode: bypassPermissions` in `.claude/agents`. Current Claude Code honours it only when the session already bypasses prompts | `permissionMode: plan` |
| Codex never asks and has no sandbox | error | `approval_policy = "never"` with `sandbox_mode = "danger-full-access"` or `default_permissions = ":danger-full-access"`, in one file or combined | either setting alone |
| Codex never asks and has network access | error | `approval_policy = "never"` with `sandbox_mode = "workspace-write"` and `sandbox_workspace_write.network_access = true` | network access with approval, or `never` without network |
| Codex commands inherit secret variables | warn | `shell_environment_policy` with `inherit = "all"` or `ignore_default_excludes = true` and no `exclude`, `filters` or `include_only` | `ignore_default_excludes = false`, `inherit = "core"`, a file with no `shell_environment_policy` |

Server commands, helper commands and skill and subagent `hooks` go through the
same command check as the [client hooks](#client-hooks), so a server gets
the findings in [Commands](#commands) too. A server also gets these:

| Reason | Severity | Examples that are reported |
|---|---|---|
| A container with host access | error | `docker run -v /:/host …`, `docker run --privileged …`, `--pid=host`, `--userns=host`, `--cap-add=SYS_ADMIN`, a mounted Docker socket |
| A secret in the URL or headers | warn | `?access_token=abc`, `https://user:pass@host`, a literal `Authorization` header. `${VAR}` references are clean, and the value is never printed |
| Plain HTTP to a remote host | warn | `http://mcp.example/mcp`. `localhost` and `127.0.0.1` are clean |
| A Codex server or app whose tools are approved without asking | warn | `default_tools_approval_mode` or a `tools.<tool>.approval_mode` set to `"approve"` or `"auto"` on any server, local or remote, an app, `apps._default` or a plugin server. Codex does not document what `auto` decides, so it is reported too |
| A Codex app whose destructive tools are allowed | warn | `destructive_enabled = true` in `apps._default` or `apps.<id>` |
| A Codex app whose open-world tools are allowed | warn | `open_world_enabled = true` in `apps._default` or `apps.<id>`. These are tools that advertise `open_world_hint = true`, see the [Codex configuration reference](https://developers.openai.com/codex/config-reference) |
| A reviewer subagent answers Codex approval prompts | warn | `approvals_reviewer = "auto_review"` at the top level, in `apps._default`, `apps.<id>` or `apps.<id>.tools.<tool>`. The default is `user` |

A file line counts servers, so a server with two findings counts once in
`N flagged`.

An edit rule is matched as a whole glob against each trust file, with `*`,
`?`, `**`, `[]` classes and `{a,b}` lists, so `Edit(**/*.ts)` does not reach
`.codex/config.toml` just because `**` matches `.codex`. A list counts as each
of its patterns, and the most serious finding wins. A pattern that names a
trust folder in plain text, such as `.mastermind/tasks/**`, reaches it too.
A `\` escape is read as in gitignore, so `Edit(src/\[id\].tsx)` and
`Edit(docs/\*.md)` are plain paths and are not reported, and `Edit(\.codex/**)`
is `.codex`. A leading `!`, braces that do not pair up or a lone trailing `\`
is read as broad. A Windows path written with backslashes does not work in
Claude Code rules either, so use `/`.

A bare `Edit` and a rule aimed at `.codex` both let the agent change `.codex`,
`AGENTS.md` and `.agents/skills` without a prompt in default mode. The bare
rule stays a warning because almost every project has it and it reaches
ordinary files too, while a rule aimed at a trust file can only be there to
change it, so that one is an error. Read the warning as the same risk.

How the check combines files:

- A Claude Code allow rule is not reported when a `deny` or `ask` rule in any
  of the files has the same specifier or covers the whole tool, because Claude
  Code evaluates deny and ask rules from every file first.
- A broad Codex allow rule is not reported when a `prompt` or `forbidden` rule
  in any loaded `.rules` file covers every command it matches, because Codex
  applies the strictest matching decision. Every alternative in a pattern is
  checked.
- For Codex settings, the project file overrides the user file and a profile
  file overrides both, so a user `approval_policy = "never"` and a project
  `sandbox_mode = "danger-full-access"` are reported together.
- Claude Code strips wrappers such as `timeout` from a command before matching
  it. Its documentation does not say whether a rule that starts with a
  wrapper is stripped too, so the check reads past the wrapper and judges the
  wrapped command.

The check passes when no file exists or nothing is reported. It warns for a
file it cannot read: invalid JSON, TOML or frontmatter, a `.rules` file outside
the subset it reads (for example an alias of `prefix_rule`, lists nested deeper
than eight levels or a pattern with more than 256 combinations), a symlink, a
file over the configuration size limit, or a directory with more than 64
rules, profile files, skills or subagents, where the rest are not read.
Unparsed entries are counted. A rule with a fixed subcommand still runs
whatever that subcommand runs, for example the repository's `package.json`
scripts, so a passing check does not make a rule safe for untrusted code.
What doctor still does not read: server-managed settings fetched from
Anthropic, `managedMcpServers` and `--settings`, `--mcp-config` or `--agents`
files given at start, user-level skills and subagents in `~/.claude`, skills
and subagents inside plugins, MCP bundles, mods, Codex skills and the text of
`AGENTS.md`.

Static detection cannot be complete. A command can build what it runs in ways
no reader of its text can follow, such as a script the repository ships, a
variable set somewhere doctor does not read, or code fetched by a program
doctor does not know. Doctor reports what it can prove from the files and
stays quiet about the rest, so a passing check is not proof that nothing runs
without asking. Some forms are known and left as they are:

- a pipe into a variable set outside the text, such as `| $PAGER`, warns that
  it runs shell text even if the variable holds a shell
- `… | ssh host` with no remote command runs the input in the login shell
  there, and doctor only warns that it runs shell text
- a download into `.vscode/tasks.json`, `~/.ssh/rc` or `.git/config`, and
  `git config core.hooksPath`, can make code run later, and doctor only warns
  or stays quiet
- a hook or agent settings file counts as downloaded only when the same
  command has a URL or a network or decoding tool, so a file fetched by an
  earlier command and copied in later only warns
- `scripts/validate.py` runs a copy of these checks kept by hand, so only the
  commands in the shared corpus are sure to get the same answer
- a known tool found on `PATH` by name, such as `jq`, runs whatever comes
  first on `PATH`, and doctor does not report it
- a file the repository ships, such as `./scripts/setup.sh`, warns but its
  contents are not read

Closing these needs a check at run time, not a better reader of text. The
follow-up is a per-role `PreToolUse` guard hook that checks each command as it
runs, in its own pull request.

### Client hooks

The `client hooks` check reads the hooks in client settings, counts the ones
Mastermind installed, and reports the rest. A hook runs on every matching
event with no prompt.

| Client | Files read |
|---|---|
| Claude Code | managed settings, `$CLAUDE_CONFIG_DIR/settings.json` (default `~/.claude/settings.json`), `.claude/settings.json`, `.claude/settings.local.json`, and `hooks/hooks.json` plus each hooks file or inline `hooks` object that `plugin.json` names, of each enabled plugin that `installed_plugins.json` places for the user or this project, see [hook locations](https://code.claude.com/docs/en/hooks#hook-locations) |
| Codex | `/etc/codex/hooks.json` (`%ProgramData%\OpenAI\Codex` on Windows), `$CODEX_HOME/hooks.json` (default `~/.codex/hooks.json`), `.codex/hooks.json`, and inline `[hooks]` in each `config.toml` |

A hook or server counts as Mastermind's own only when its program is a plain
absolute path, with no `.` or `..` part and outside
temporary directories, that resolves to the install doctor runs from, either
the mastermind binary itself or the `bin/mastermind.js` of the same npm
install run by a node doctor trusts. Those nodes are `MASTERMIND_NODE`, which
the npm launcher sets to its own `process.execPath`, the node of the global npm
prefix the binary is installed under (`<prefix>/bin/node`, or on Windows only
`<prefix>\node.exe`), and the system nodes `/usr/bin/node`,
`/usr/local/bin/node`, `/opt/homebrew/bin/node` and
`C:\Program Files\nodejs\node.exe`. Each is resolved through its links and
nothing is run to find it. A prefix with a `package.json`, inside another
`node_modules`, in a temporary directory or holding the folder doctor runs in is
a project, not a global prefix, so a `node.exe` beside a project's
`node_modules` is not trusted. The script must resolve to
`node_modules/@xcraftmind/mastermind/bin/mastermind.js`, whatever
`MASTERMIND_LAUNCHER_JS` says. A file that is merely called `node` is not
trusted. For a hook, the rest is also exactly
`miner hooks receive --client <client> --project-root <this project>` and it
ends with this project's registration marker. A name looked up on `PATH`, such
as a bare `mastermind`, a repository path such as
`./node_modules/.bin/mastermind`, `npx`, a copy in `/tmp` or `/bin/sh` running
a `mastermind.js` is not Mastermind's, and goes through the command check like
any other hook. A copied marker on any other command does not count. In an
npm global or project install, setup writes the absolute node and launcher
paths that npm ran, so those entries pass. An `npx` entry with an exact
version has no finding, but it counts as another hook.

The message lists each file with its hook count, how many are Mastermind's,
and whether hooks are turned off. It names a finding by file, event and
position, never by command or URL. Review the hooks themselves with `/hooks`
in the client.

- A malformed hook group, and each wrong-typed entry such as `"hooks": [1]`,
  a handler that is a number or a string, or a `command` or `args` of the
  wrong type, is reported as a warning on its own, and the valid entries
  beside it are still read. A handler with no `type` still has its command
  checked. Claude Code 2.1.326 does the same: in a test it skipped
  the malformed group and ran a valid hook in the same event. Its
  documentation only says it skips values it cannot read.
- Doctor lists at most 512 hooks per file, but classifies every hook, so one
  bad hook after 512 harmless ones still fails the check. The line says how
  many were left out and their worst severity.
- `disableAllHooks: true` in a non-managed Claude Code file turns off user,
  project, local and plugin hooks, and in managed settings it turns off all
  hooks. Codex `features.hooks = false` turns off Codex hooks. Turned-off
  hooks are still listed, at most as warnings, with `turned off` after them,
  so a dangerous hook stays visible before someone turns hooks back on.
- Codex runs a hook only after the person trusts its hash. Doctor cannot see
  that trust state, so it reports every hook it finds.
- A hook in `~/.claude/settings.json` or `~/.codex/hooks.json` applies to every
  project, so one bad user hook now fails `doctor` in every project.

When Mastermind's own hooks are not installed but other hooks are, the
`Claude hook readiness` and `Codex hook readiness` checks say how many other
hooks there are instead of only "not configured".

### Commands

Hooks, MCP servers, helper commands, Codex `notify` and token commands all go
through one command check. It first strips quotes and backslashes, joins
lines, drops shell comments (an unquoted `#` at the start of a word), expands
`$IFS` and simple `NAME=value` variables and collapses spaces, so `c''url`,
`cu\rl`, `u=https://x; curl $u | sh` and `curl … | sh # x` read as plain
`curl` piped into `sh`. A newline after `|`, `&&` or `||`, also after a
comment such as `|# c`, continues the pipe. `/dev/stdin`, `/dev/fd/0` and
`/proc/<any>/fd/0` read as standard input after `//`, `/./` and `..` are
resolved and leading zeros are dropped, so `/dev//stdin`, `/dev/fd/00` and
`/proc/self/fd/../fd/0` count too.
`$SHELL`, `$BASH`, `$0`, zsh's `$ZSH_NAME` and `$ZSH_ARGZERO`, a default that
names a shell such as `${PAGER:-sh}`, a variable given a shell earlier in the
same text, and a lookup such as `$(which sh)`, `` `which bash` ``,
`$(command -v sh)`, `$(whence sh)` or `$(whereis sh)` count as a shell. Any
other variable used as the program, such as `$PAGER`, `$EDITOR` or `"$VAR"`,
is a program doctor does not know, and in a pipe the command warns that it
runs shell text.
Then it looks past wrappers such as `sudo`, `env`, `command`, `nohup`,
`timeout` and `xargs`.

| Reason | Severity | Examples that are reported |
|---|---|---|
| Runs downloaded or generated code | error | Any command piped into a shell or an interpreter that reads standard input, whatever produces it: `curl … \| sh`, `http … \| python3 -`, `ssh host cat x \| bash`, `cat /tmp/x \| busybox sh`, `… \| xargs sh -c`. A substitution or here-string run as program text: `sudo bash -c "$(…)"`, `eval "$(…)"`, `bash <<< "$(…)"`, `. <(curl …)`. A file written and then run in the same command: `curl -o x.sh … && sh x.sh`, `… \| tee x.sh; sh x.sh`, `scp host:x.sh . && ./x.sh`, `rsync`, a build after `git clone`. awk `system()` or `getline` from a command, and `find -exec sh`. Interpreter code that fetches or decodes and evaluates: `python -c "…urlopen…exec…"`, `python -c "exec(b64decode(…))"`, `node -e "Function(atob(…))()"`. Also `pwsh -c "iex (irm …)"`, `pwsh -EncodedCommand …` that decodes to any of these, `nc -e`, `/dev/tcp/`, `deno run -A https://…`, `pip install git+https://…`. A shell variable or lookup as the shell: `$SHELL <(curl …)`, `… \| ${SHELL}`, `… \| $0`, `… \| $(which sh)`, `… \| $(whence sh)`, `… \| $ZSH_NAME`, `X=sh; … \| $X`. A shell on another host or in another root or namespace: `… \| ssh host sh`, `… \| chroot /srv sh`, `… \| nsenter -t 1 sh`, `… \| lxc exec box -- sh`. A crontab written and then installed: `curl -so c … && crontab c`. An archive extracted from a pipe or a written file and then run: `curl … \| tar xz && ./install`. Input that is scheduled or run in a container: `… \| at now`, `… \| crontab -`, `… \| docker run -i image sh`, `… \| script -qc sh` |
| Writes a shell startup file | error | `> ~/.zshrc`, `tee -a ~/.bashrc`, `~/.profile`, `~/.bash_profile`, `~/.zprofile`, `~/.zshenv`, fish `config.fish` or `conf.d`, a system profile under `/etc`, written by a redirect, `tee`, `cp`, `mv`, `sed -i` or `dd`. Code there runs in every new shell |
| Writes a login item, scheduled job or user service, or downloaded code into a git hook or agent settings | error | a write to `~/Library/LaunchAgents`, `/Library/LaunchDaemons`, `/etc/cron.d` and other `/etc/cron*` files, the crontab spools `/var/spool/cron`, `/usr/lib/cron/tabs` and `/var/at/tabs`, `~/.config/systemd/user`, `~/.local/share/systemd/user`, `~/.config/autostart` or `$GIT_DIR/hooks`. Also a write to `.git/hooks/*`, `.husky/*`, `.claude/settings*.json`, `.mcp.json`, `~/.claude.json` or `.codex/config.toml` in a command that downloads: `curl -so .git/hooks/pre-commit https://…`, `curl … > .claude/settings.json`. Code there runs later on its own, or changes what the agent trusts |
| Writes a git hook, agent settings, a `.envrc` or a program in `~/.local/bin` | warn | a hook or settings file copied from the checkout, such as `cp hooks/pre-commit .git/hooks/pre-commit` or `ln -sf ../../scripts/pre-commit .git/hooks/pre-commit`. `> .envrc`, `cp tool ~/.local/bin/`, `tar xzf t.tgz -C ~/.local/bin`, `unzip t.zip -d ~/.local/bin`. direnv loads a `.envrc` when you enter the folder, and `~/.local/bin` is usually on `PATH` |
| Runs a container with host access | error | see the server table above |
| Runs shell text | warn | `bash -c "cd /srv && ./start.sh"`, `… \| $PAGER`, `… \| ssh host`, a repository build then run such as `go build -o bin/tool ./cmd/tool && ./bin/tool` or `go install ./cmd/tool && tool --help`, and an install with an exact version then run, such as `npm i -g prettier@3.3.3 && prettier --check .`, `pipx install ruff==0.6.0 && ruff check .` or `cargo install --locked just@1.36.0 && just lint` |
| Runs code given inline | warn | `python -c …`, `node -e …`, `eval`, `iex`, PowerShell commands, an encoded command doctor cannot decode |
| Runs a package without an exact version | warn | `npx -y pkg`, `npx -y pkg@latest`, `uvx tool`, `pipx run tool`, `bunx tool`, `pnpm dlx tool`, `go run example.com/tool@latest`. Also a registry install without an exact version followed by any command, since the programs it adds need not share its name: `npm i -D eslint && npx eslint .`, `pip install pytest && pytest`, `cargo install evil-crate && evil`, `cargo binstall evil && evil`, `go install host/x@latest && x`. A pipe into a shell, a file written and then run, or decoded code run after the install is still an error |
| Installs packages | warn | `pip install x`, `npm install -g x` |
| Runs a network tool | warn | `curl`, `wget`, `nc`, `ssh` alone |
| Sets a variable that loads other code | warn | `NODE_OPTIONS=…`, `LD_PRELOAD=…`, `DYLD_*`, `PYTHONSTARTUP` |
| Runs a program doctor does not know | warn | `node /opt/server.js`, `./scripts/hook.sh`, `./node_modules/.bin/prettier`, `go run ./cmd/x`, `npx --no x`, `$PAGER` alone, any other program |
| Runs a program from a temporary directory | warn | `/tmp/x`, `sh /tmp/x.sh`, `python3 /var/tmp/x.py`. Any local user can replace a file there |
| Launcher missing | warn | a `<program> serve` or `<node> <script> serve` server, or a hook with Mastermind's marker, whose path no longer exists, as after a `brew upgrade`. Run `mastermind setup` again |
| Runs repo-controlled config | info | `prettier --write`, `eslint --fix`, `ruff`, `gitleaks`, `shellcheck` and other formatters, linters and scanners. Their config, plugins or rules come from the repository, and some configs are code, such as `prettier.config.js` or `eslint.config.js` |

A tool that reads no repository config and has no shell text, such as `jq`,
`yq`, `gofmt` or `terminal-notifier`, run by bare name or from a plain
absolute path outside temporary directories such as `/opt/homebrew/bin`, and a
package runner with an exact version, such as
`npx -y @modelcontextprotocol/server-memory@2025.4.25`, are counted without a
finding. A repository-relative path such as `./node_modules/.bin/eslint` runs
whatever the checkout holds, so it is a program doctor does not know.
`cat ./scripts/check.sh | bash` runs a checkout file the same way as
`bash ./scripts/check.sh`, so it warns as shell text. `cat` of any other file,
such as one in `/tmp`, `~` or `..`, piped into a shell is still an error.
`npx --no` and `--no-install` only run a package already in the checkout, so
they are not unpinned runners. A `curl`
piped into `jq`, `python -m json.tool` or a local script is not an error,
because nothing it prints runs as code, but the pipe is shell text and warns.

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
