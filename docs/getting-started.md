# Getting started

This guide installs Mastermind, sets up one repository and its clients, and
shows how to update and remove it. The first setup takes a few minutes. Most of
it is the first index of your code and documentation.

You need Node.js 24 or later for the npm package. Run these commands inside your
repository:

```bash
npm install -g @xcraftmind/mastermind
mastermind init
```

The first `init` picks the client you run it from, or the Claude Code and Codex
clients it finds installed. For each selected client it writes the workflow
instructions, registers the `mmcg` MCP server, installs the bundled workflows,
turns on profile access and task mining, and indexes the repository. It saves
these choices in `.mastermind/setup.json`, and a repeated `init` reuses them.

Restart the client after setup and accept any trust prompt it shows. Mastermind
can't grant client trust for you. To build from source instead, see
[other installation methods](#other-installation-methods).

## 1. Choose the clients

| Run | What `init` selects |
|---|---|
| Inside Claude Code or Codex | The active client, when its CLI is available |
| Outside an agent | The installed native clients. A terminal asks if it finds none |
| Unattended, with no clients installed | Local scaffold and index only |
| `--client claude`, `codex` or `all` | Those clients, with profile access and task mining |
| `--client none` | Local setup with no client integration |
| `--dry-run --json` | The saved choices and the planned steps, with no writes and no client calls |

```bash
mastermind init --client codex
mastermind status --json
```

`status --json` shows the index, the client configuration, the sessions it has
seen and any missing evidence. A saved setting doesn't prove a live client
connection, so check the client itself after the restart.

If a component fails, `init` names it. Fix that issue and run `init` again. A
failed refresh of the optional Git observations for your profile shows as
`degraded` with the reason, and the rest of the setup still succeeds. An
existing `CONTEXT.md` stays unless you pass `--force`.

### The workflow block in CLAUDE.md and AGENTS.md

With the workflow on, `init` keeps its instructions in a managed block between
`<!-- mastermind:workflow:begin ... -->` and `<!-- mastermind:workflow:end -->`.
Claude gets it in `CLAUDE.md`, Codex in `AGENTS.md`.

| File state | What `init` does |
|---|---|
| Missing | Writes a project header for you to fill in, then the block |
| Present, no block | Appends the block. Your text stays |
| Older block | Replaces only the block. Run `init` after each update |
| Unmarked `## Mastermind workflow` section from a 3.x `init` | Replaces that section, up to the next `#` or `##` heading, with the block. Keeps the old file as `CLAUDE.md.mastermind-backup` or `AGENTS.md.mastermind-backup` and prints a warning. Copy back anything you changed in that section |
| Any state, with `--force` | Replaces the whole file and keeps a backup |

`--workflow off` writes no block. `mastermind doctor` checks the file of each
saved client and warns when the block is missing or from another version.

## 2. Choose mining and profile access

Mining turns your sessions into proposed working preferences. Nothing becomes an
active habit until you review it.

| Option | Effect | Model calls |
|---|---|---|
| `--mining off` | No capture, and the managed miner stops | None from mining |
| `--mining capture` | Records local interaction evidence for later | None, unless the refiner is on |
| `--mining task` | The current agent proposes candidates, saved locally after the session ends cleanly. Default on macOS and Linux | No separate model. The extra context and tool calls count against the current client's usage |
| `--mining on --provider native` | Capture plus bounded mining in a separate process | Each captured client's CLI and active model |
| `--profile-access on` | Lets the selected clients read this project's view of your profile | None |
| `--refiner on --provider native` | Rewrites your prompts through the client and model that captured them | Extra calls, outside the miner budget |

Selected clients get profile access by default, and `--profile-access off` is
saved for later `init` runs. The refiner is off by default and needs
`--mining capture`, `task` or `on`. The profile delivers authored Git
observations and reviewed rules. [Persona hooks](guides/persona-hooks.md)
walks through review, and [managed workers](reference/persona.md#managed-workers)
covers the budgets of the separate miner.

To switch an existing repository and keep its other choices:

```bash
mastermind init --mining task
```

Restart the client so its MCP server exposes the new submission tool.

## 3. Look at a change

```bash
mastermind impact --since main
mastermind ui --since main
```

Replace `main` with a Git baseline that exists. Lens serves on loopback and shows
change impact, architecture and context profiles. Missing or withheld profile
data is marked, and private profile data never goes into a standalone export.

| Need | Command |
|---|---|
| Refresh the index | `mastermind index .` |
| Reparse every file | `mastermind index . --force` |
| Keep the index fresh for CLI and Lens use | `mastermind watch` |
| Repository map | `mastermind map .` |
| Compact briefing for a role | `mastermind brief --role planner --since main --budget-tokens 2000` |
| Combined context preview | `mastermind context preview --role planner --since main --query "service boundaries"` |
| Diagnose the project setup | `mastermind doctor` |
| Offline review package | `mastermind review export --since main --out mastermind-review` |

Discovery follows your ignore rules. Code, documentation and history each have
their own freshness check. A context preview shows revisions and omissions, but
it can't show whether a model used them. The export writes offline HTML, SARIF,
a summary, an evidence manifest and a GitHub Actions workflow into a new
directory, see [review export](reference/lens.md#pr-evidence-package-mastermind-review-export).

To run a task through approval, checks, audit and review, follow the
[workflow guide](workflow.md). Indexing and MCP work without it.

## 4. Update

```bash
mastermind update
```

If you already upgraded with npm directly, run `mastermind update --workflow-only`
once to refresh the installed workflows. Project hooks call the package
launcher, so the repositories don't need another `init`.

| Option or condition | Behavior |
|---|---|
| Global or project npm install it can prove | Updates in that same scope, runs the new workflow installer, then checks the native binary version and the workflow files |
| `--dry-run --json` | Local read-only plan, with no network calls and no install |
| `--workflow-only` | Refreshes workflows from the current package without updating the package |
| `--client claude`, `codex` or `all` | Picks the clients. Without it, the installed ownership manifests decide |
| `--profile core`, `frontend`, `security` or `full` | Changes the workflow profile. Without it, each installed profile stays |
| Workflow entry you edited, linked or that Mastermind did not install | Never overwritten. The update stops before any write, lists every such entry with its kind and path, then prints one `Next:` step |
| `--move-aside` | Moves each listed entry to `<client root>/.mastermind-preserved/<time>/<kind>/<name>`, where `<kind>` is `edited`, `linked`, `unowned` or `unsupported_entry`, then installs the bundled copies |
| npx, manual or unknown install | Returns `manual_required` with the steps to do by hand |
| Failure after a step | Reports a partial result with recovery steps |

Restart the client after a workflow update. To check the installed workflows on
their own, run `mastermind doctor --workflow --client all --json`.

The client root is `~/.claude` or `~/.codex`. `.DS_Store`, `__pycache__` and
`*.pyc` files inside a workflow folder don't count as edits. To keep an edited
skill, compare it with the bundled copy first. The `Next:` line names the
folder that holds the bundled copies. Then run `mastermind update --move-aside`,
which moves your copy out of `skills/` and `agents/`. A copy left there makes
the client load two artifacts with one name.

### Update from 4.0.x

The first `mastermind update` from 4.0.x runs the 4.0.x updater. It refuses
`--move-aside` as an unknown argument, and it stops at the first edited,
linked or unowned workflow entry before it updates the package. If it stops:

1. Move each named entry out of `~/.claude/skills`, `~/.claude/agents` and
   `~/.codex/skills`, for example:

   ```bash
   mkdir -p ~/.claude/.mastermind-preserved
   mv ~/.claude/skills/mastermind-task-planning ~/.claude/.mastermind-preserved/
   ```

2. Run `mastermind update` again. Repeat until it finishes.

Once the package is updated, later updates list every entry at once and
`--move-aside` moves them for you.

## Setup options

| Option | Effect |
|---|---|
| `--workflow on` or `off` | Saves whether `init` installs the bundled workflows |
| `--no-global` | Same as `--workflow off` |
| `--draft-with claude` | Lets Claude draft the new scaffold documents. It may edit files and makes model calls |
| `--seed-style` | Seeds your profile from this repository's authored Git history |
| `--no-index` | Skips this index refresh |
| `--force` | Replaces the scaffold documents and keeps backups. Client customizations stay protected |

The bundled workflows default to on when `init` runs through the npm `mastermind`
command, and to off when you run a cargo-built `mmcg init`. Scaffold drafting is
off by default. `--workflow off` doesn't change MCP or mining. The task runtime
budgets (`--exec-timeout`, `--review-timeout`, `--worktree-max-total-bytes` and
the others) are in [task runtime](reference/task-runtime.md).
`mastermind init --help` lists every option.

## Other installation methods

| Platform | Support |
|---|---|
| macOS and Linux | Local tools, native hooks, managed mining, native task execution |
| Windows | Local indexing and client setup with `--mining off`. No native hooks and no managed miner |

| Method | Commands | Needs |
|---|---|---|
| Global npm | `npm install -g @xcraftmind/mastermind`, then `mastermind init` | Node.js 24+ |
| Project npm | `npm install -D @xcraftmind/mastermind`, then `npx mastermind init` | Node.js 24+ |
| Cargo from crates.io | `cargo install mmcg --locked`, then `mmcg init --workflow off` | Rust 1.96+ |

Cargo installs only the native `mmcg` binary, without the npm workflow bundle
and without `update`, `install`, `list` or `doctor --workflow`.

### Current source build

From a checkout of this repository:

```bash
cargo install --path mcp/servers/mmcg --locked
```

Then, inside the repository you want to work on:

```bash
mmcg init --workflow off
```

To set up one piece at a time, use `mastermind setup` for MCP registration,
`mastermind miner hooks` for capture and `mastermind install` (npm only) for
workflows. The client guides cover scopes and removal:
[Claude Code](integrations/claude-code.md), [Codex](integrations/codex.md) and
[other MCP clients](integrations/generic-mcp.md).

## Where things are stored

| Location | Contents |
|---|---|
| `.mastermind/setup.json` | The saved project choices. It grants no permission by itself |
| `.mastermind/mmcg.db` | Code and documentation index |
| `.mastermind/tasks/` | Specs, checks, execution, audit and review records |
| `CONTEXT.md` | Project knowledge you maintain |
| `~/.mastermind/` | Optional personal profile, capture journal and worker records |
| Client configuration | MCP registration and workflow adapters |

`init` writes `.mastermind/.gitignore`, so your own `.gitignore` doesn't need a
`.mastermind/` line. To commit task contracts and executor reports, for review
or the [GitHub audit Action](github-action.md#commit-task-evidence), uncomment
its `!tasks` lines and keep the rest ignored.

| Operation | Model access |
|---|---|
| Default scaffold, indexing, queries, Lens, export | None |
| Task mining | The current agent, no separate request |
| `init --draft-with claude` | Claude, for document drafting |
| Mining or refinement with a provider | Captured episodes or prompts go to the selected provider |
| Native task execution and review | Claude Code, when you run them |

`mastermind --help` prints the short command list, and `mmcg --help` the full
native one.

## Remove from a project

Run this in the project root. The first command shows the plan, the second
applies it:

```bash
mastermind uninstall
mastermind uninstall --force
```

| Removed | Kept |
|---|---|
| Hooks, capture grant, worker and profile access of each client in this project | `CONTEXT.md`, `CLAUDE.md` and `AGENTS.md`, including the workflow block |
| `.mastermind/`, with the task records | Workflow skills and agents in `~/.claude` and `~/.codex`, which other projects use |
| The Claude entry in the project `.mcp.json` | Other clients' MCP entries and `~/.mastermind/` |

Copy `.mastermind/tasks/` first if you need the task records. `--scope all` also
removes the global Claude registration. Remove the Codex registration with
`mastermind setup codex --remove --write`.
