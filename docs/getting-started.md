# Getting started

At the end of this guide your repository is indexed, Claude Code or Codex is
connected to it, and the Mastermind workflows are installed. It takes a few
minutes, mostly for the first index.

## Set up a repository

1. Install the package with Node.js 24 or later, then set up the repository
   from its root:

   ```bash
   npm install -g @xcraftmind/mastermind
   mastermind init
   ```

   For each selected client, `init` writes the workflow block, registers the
   `mmcg` MCP server, installs the bundled skills, turns on profile access and
   task mining, and indexes the repository. It saves these choices in
   `.mastermind/setup.json`, and a repeated `init` reuses them. If a component
   fails, `init` names it, and you run `init` again after the fix. A failed
   refresh of the optional Git observations for your profile only marks that
   part `degraded`.

2. Restart the client, accept its trust prompt and review the hooks in
   `/hooks`. Mastermind can't grant client trust for you.

3. Check the result with `mastermind status --json` and `mastermind doctor`.
   `status` shows the index, the client configuration and the sessions it has
   seen. `doctor` checks the setup and the MCP handshake, but only the client
   itself shows a live connection.

## Choose the clients

| Run | What `init` selects |
|---|---|
| Inside Claude Code or Codex | The active client, when its CLI is available |
| Outside an agent | The installed native clients. A terminal asks if it finds none |
| Unattended, with no clients installed | Local scaffold and index only |
| `--client claude`, `codex` or `all` | Those clients, with profile access and task mining |
| `--client none` | Local setup with no client integration |
| `--dry-run --json` | The saved choices and the planned steps, with no writes and no client calls |

Claude Code gets skills, named subagents and the workflow block in `CLAUDE.md`.
Codex gets the same skills and the block in `AGENTS.md`, without subagent
files. A fresh install uses the `core` skill profile, and
`mastermind update --workflow-only --client claude --profile frontend` switches
it to `frontend`, `security` or `full`.

## Choose mining and profile access

Mining turns your sessions into proposed working preferences, and nothing
becomes an active habit until you review it in
[persona hooks](guides/persona-hooks.md). To change one choice and keep the
others, run `init` with only that option, then restart the client.

| Option | Effect | Model calls |
|---|---|---|
| `--mining off` | No capture, and the managed miner stops | None from mining |
| `--mining capture` | Records local interaction evidence for later | None, unless the refiner is on |
| `--mining task` | The current agent proposes candidates, saved after the session ends cleanly. Default on macOS and Linux | No separate model. The extra context and tool calls count against the client's usage |
| `--mining on --provider native` | Capture plus bounded mining in a separate process | Each captured client's CLI and model |
| `--profile-access on` or `off` | Lets the selected clients read this project's view of your profile. On by default | None |
| `--refiner on --provider native` | Rewrites your prompts through the client that captured them. Needs `--mining capture`, `task` or `on` | Extra calls, outside the miner budget |
| `--workflow on` or `off` | Whether `init` installs the bundled workflows. On through npm, off for a cargo-built `mmcg init`. `--no-global` is the same as `off` | None |
| `--draft-with claude` | Claude drafts the new scaffold documents and may edit files | Claude |
| `--seed-style` | Seeds your profile from this repository's authored Git history | None |
| `--no-index` | Skips this index refresh | None |
| `--force` | Replaces the scaffold documents, `CONTEXT.md` included, and keeps backups. Client customizations stay protected | None |

`mastermind init --help` lists every option, including the task runtime budgets
in [task runtime](reference/task-runtime.md). `mmcg --help` lists the full
native command set.

## The workflow block in CLAUDE.md and AGENTS.md

`init` keeps its instructions between `<!-- mastermind:workflow:begin ... -->`
and `<!-- mastermind:workflow:end -->`. `--workflow off` writes no block, and
`mastermind doctor` warns when a saved client's block is missing or from another
version.

| File state | What `init` does |
|---|---|
| Missing | Writes a project header for you to fill in, then the block |
| Present, no block | Appends the block. Your text stays |
| Older block | Replaces only the block. Run `init` after each update |
| Unmarked `## Mastermind workflow` section from a 3.x `init` | Replaces that section, up to the next `#` or `##` heading, with the block. Keeps the old file as `CLAUDE.md.mastermind-backup` or `AGENTS.md.mastermind-backup` and warns. Copy back anything you changed in that section |
| Any state, with `--force` | Replaces the whole file and keeps a backup |

## Register MCP without init

`setup` previews the change, and `--write` applies it. It keeps unrelated
entries. A customized entry needs `--force` with `--write`, which saves a
backup under `~/.mastermind/setup-backups/`. To remove a registration, run the
same command with `--remove`.

| Client | Command | Where it goes |
|---|---|---|
| Claude Code, user scope (default) | `mastermind setup claude --scope user` | Native `claude mcp` registration |
| Claude Code, project scope | `mastermind setup claude --scope project --root .` | The repository's `.mcp.json` |
| Codex, user scope only | `mastermind setup codex --scope user` | `codex mcp`, then a check of the enabled `mmcg` entry. `doctor` reads it from `[mcp_servers.mmcg]` in `~/.codex/config.toml` |
| Cursor, Continue, any JSON client | See [other MCP clients](integrations/generic-mcp.md) | |

Capture hooks and workflow skills have their own commands:
`mastermind miner hooks setup --client claude --project-root . --write` and,
from npm only, `mastermind install --client claude`.

## Update

Run `mastermind update`, then restart the client. It updates the package in the
npm scope it can prove, global or project, refreshes the installed workflows and
checks the binary version and the workflow files. Project hooks call the package
launcher, so repositories don't need another `init`.

| Option or condition | Behavior |
|---|---|
| `--dry-run --json` | Local read-only plan, with no network calls and no install |
| `--workflow-only` | Refreshes workflows from the current package. Run it once if you upgraded with npm directly |
| `--client claude`, `codex` or `all` | Picks the clients. By default the installed ownership manifests decide |
| `--profile core`, `frontend`, `security` or `full` | Changes the workflow profile. By default each installed profile stays |
| A workflow entry you edited, linked or that Mastermind didn't install | Never overwritten. The update stops before any write, lists every such entry with its kind and path, and prints one `Next:` step |
| `--move-aside` | Moves each listed entry to `<client root>/.mastermind-preserved/<time>/<kind>/<name>`, where `<kind>` is `edited`, `linked`, `unowned` or `unsupported_entry`, then installs the bundled copies |
| npx, manual or unknown install | Returns `manual_required` with the steps to do by hand |
| Failure after a step | Reports a partial result with recovery steps |

The client root is `~/.claude` or `~/.codex`, and `.DS_Store`, `__pycache__` and
`*.pyc` files don't count as edits. To keep an edited skill, compare it with the
bundled copy the `Next:` line points to before `--move-aside`. A copy left in
`skills/` or `agents/` makes the client load two artifacts with one name.
`mastermind doctor --workflow --client all --json` checks the installed
workflows on their own.

The first update from 4.0.x still runs the 4.0.x updater, which has no
`--move-aside` and stops at the first edited, linked or unowned entry. Move that
entry out of `~/.claude/skills`, `~/.claude/agents` or `~/.codex/skills` and run
`mastermind update` again until it finishes:

```bash
mkdir -p ~/.claude/.mastermind-preserved
mv ~/.claude/skills/mastermind-task-planning ~/.claude/.mastermind-preserved/
```

## Other installation methods

| Method | Commands | Needs |
|---|---|---|
| Project npm | `npm install -D @xcraftmind/mastermind`, then `npx mastermind init` | Node.js 24+ |
| Cargo from crates.io | `cargo install mmcg --locked`, then `mmcg init --workflow off` | Rust 1.96+ |
| Source checkout | `cargo install --path mcp/servers/mmcg --locked` in the checkout, then `mmcg init --workflow off` in your repository | Rust 1.96+ |

Cargo installs only the native `mmcg` binary, without the workflow bundle and
without `update`, `install`, `list` or `doctor --workflow`. On Windows you get
local indexing and client setup with `--mining off`, but no native hooks and no
managed miner.

## Where things are stored

| Location | Contents |
|---|---|
| `.mastermind/setup.json` | The saved project choices. It grants no permission by itself |
| `.mastermind/mmcg.db` | Code and documentation index, rebuildable from the files |
| `.mastermind/tasks/` | Specs, checks, execution, audit and review records, and `_lessons.md` with reusable lessons |
| `CONTEXT.md` | Project decisions you maintain |
| `~/.mastermind/` | Optional personal profile (`style.db` and a readable `style.md`), capture journal and worker records |

`init` writes `.mastermind/.gitignore`, so your own `.gitignore` needs no
`.mastermind/` line. To commit task contracts and executor reports, for review
or the [GitHub audit Action](github-action.md#commit-task-evidence), uncomment
its `!tasks` lines.

Indexing, queries, Lens and export make no model calls. Separate model requests
come only from `--mining on` and the refiner, which send captured episodes or
prompts to the selected provider, from `--draft-with claude`, and from native
task execution and review, which run Claude Code when you start them.

## Remove from a project

In the project root, `mastermind uninstall` shows the plan and
`mastermind uninstall --force` applies it. Copy `.mastermind/tasks/` first if
you need the task records.

| Removed | Kept |
|---|---|
| Hooks, capture grant, worker and profile access of each client in this project | `CONTEXT.md`, `CLAUDE.md` and `AGENTS.md`, including the workflow block |
| `.mastermind/`, with the task records | Workflow skills and agents in `~/.claude` and `~/.codex`, which other projects use |
| The Claude entry in the project `.mcp.json` | Other clients' MCP entries and `~/.mastermind/` |

`--scope all` also removes the global Claude registration. Remove the Codex
registration with `mastermind setup codex --remove --write`.
