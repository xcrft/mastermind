# Getting started

Current-task mining requires Mastermind 3.2.0 or newer. For local development,
use a [source build](#current-source-build).

Run these commands inside your repository:

```bash
npm install -g @xcraftmind/mastermind
mastermind init
```

Requires Node.js 24+. The first `init` detects the active client, or installed
Claude Code and Codex clients. It creates project guidance, indexes code and
documentation, installs workflows, enables profile delivery and mines
preferences in the current agent task. Choices are saved in `.mastermind/setup.json`.

## 1. Choose your setup

| First run | Behavior |
|---|---|
| Inside Claude Code or Codex | Select the active client when its CLI is available |
| Outside an agent | Select installed native clients; ask in a terminal if none were found |
| Unattended, no installed clients | Local scaffold and index only |
| `--client claude`, `codex`, or `all` | Configure the selected clients with profile access and task mining |
| `--client none` | Local project setup without client integration |
| `--dry-run --json` | Show selected settings and proposed steps without writes or client calls |
| Repeated `init` | Reuse saved choices and reconcile the requested components |

For an explicit setup:

```bash
mastermind init --client codex
mastermind status --json
```

Restart the client after setup. Complete any trust prompt in that client.
Mastermind does not grant native client trust.

An incomplete setup reports failed components. Fix the reported issue and repeat
`init`. Existing `CONTEXT.md` and `CLAUDE.md` are preserved unless `--force` is used.
Use `status --json` to inspect configuration, observed sessions and missing
evidence. Saved configuration alone does not establish a live client connection.

## 2. Select mining and profile access

| Option | Effect | Provider calls |
|---|---|---|
| `--mining off` | Disable capture and stop the managed miner | None from mining |
| `--mining capture` | Record local interaction evidence for later inspection | None unless the refiner is enabled |
| `--mining task` (macOS/Linux default) | The current agent proposes candidates, saved locally after complete Stop | No separate model invocation. Extra task context and tool use consume the current client's usage |
| `--mining on --provider native` | Capture and start bounded semantic mining | Uses each captured client and its active model through the native CLI |
| `--profile-access on` | Allow the selected clients to read this project's personal-profile view | None from the grant |
| `--refiner on --provider native` | Refine user prompts through their captured native client and model | Extra calls outside the miner budget |

Selected clients receive profile access by default. Prompt refinement defaults
to off and requires `--mining capture`, `task` or `on`. Profile delivery includes
authored Git observations and reviewed rules; new mined drafts require review
before becoming active habits. `--profile-access off` preserves that opt-out on
future init runs. See
[Persona hooks](guides/persona-hooks.md) for evidence and review boundaries.

To switch an existing repository while preserving its other saved choices:

```bash
mastermind init --mining task
```

Restart the client so its MCP server exposes the new submission tool. Existing
saved background-mining choices remain unchanged until explicitly switched.
For separate analysis, see [managed workers](reference/persona.md#managed-workers).
Its call/runtime budgets and automatic renewal apply only to that mode.

## 3. Inspect the repository

```bash
mastermind impact --since main
mastermind ui --since main
```

Replace `main` with an existing Git baseline. Lens serves on loopback and shows
change impact, architecture, and context profiles. Missing or withheld profile
data is explicit, and private personal data stays out of standalone exports.

| Need | Command |
|---|---|
| Refresh the index | `mastermind index .` |
| Reparse discovered files | `mastermind index . --force` |
| Refresh while a process runs | `mastermind watch` |
| Repository map | `mastermind map .` |
| Compact briefing | `mastermind brief --role planner --since main --budget-tokens 2000` |
| Combined context | `mastermind context preview --role planner --since main --query "service boundaries"` |
| Diagnose project state | `mastermind doctor` |

Discovery follows ignore rules. Code, documentation, and history have separate
freshness checks. Context previews show revisions and omissions, not proof that
a model received or used them.

## 4. Update

```bash
mastermind update
```

| Mode or condition | Behavior |
|---|---|
| Proven global or project npm installation | Update in that same scope, then run the new workflow installer and verify the native binary version and workflow files |
| `--dry-run --json` | Local read-only plan without network calls or installation |
| `--workflow-only` | Refresh workflows from the current package without updating the package or native binary |
| `--client claude`, `codex`, or `all` | Select clients explicitly, otherwise use valid installed ownership manifests |
| `--profile core`, `frontend`, `security`, or `full` | Change the workflow profile, otherwise preserve each installed selection |
| Locally edited or conflicting workflow files | Block overwrite and report the conflict |
| npx, manual, or unknown installation | Return `manual_required` with instructions instead of guessing an npm scope |
| Failure after an update step | Report a partial result and recovery instructions |

Inspect configuration independently with
`mastermind doctor --workflow --client all --json`. Restart the client after
workflow updates.

## Optional setup controls

| Option | Effect |
|---|---|
| `--workflow on\|off` | Save whether `init` installs bundled workflows for selected clients |
| `--no-global` | Alias for `--workflow off` |
| `--draft-with claude` | Explicitly use Claude to draft new scaffold documents, which may edit files and use provider calls |
| `--seed-style` | Explicitly seed Git observations during scaffolding |
| `--no-index` | Skip this index refresh |
| `--force` | Replace scaffold documents with backups, while client customization conflicts remain protected |

Bundled workflows default to on for selected clients. Scaffold drafting is off.
Profile-enabled client setup refreshes authored Git observations locally.
`--workflow off` leaves MCP and mining choices separate. See
`mastermind init --help` for all options.

## Optional: use task control

```bash
mastermind new-spec "Add account recovery"
```

Fill the goal, scope, acceptance criteria, and checks before approval.
[Workflow](workflow.md) covers checks, audit, semantic review, and completion.
Indexing and MCP can also be used without task control.

## Optional: export a review

```bash
mastermind review export --since main --out mastermind-review
```

The new directory contains offline HTML, SARIF, a summary, an evidence manifest,
and a GitHub Actions workflow. Existing output paths are rejected. See the
[export reference](reference/mmcg.md#pr-evidence-package-mmcg-review-export).

## Platforms and alternative installations

| Platform | Support |
|---|---|
| macOS and Linux | Local tools, native hooks, and managed mining |
| Windows | Local indexing and client setup with `--mining off`. Native hooks and the managed miner are unsupported |

| Method | Commands | Requirement |
|---|---|---|
| Global npm | `npm install -g @xcraftmind/mastermind`, then `mastermind init` | Node.js 24+ |
| Project npm | `npm install -D @xcraftmind/mastermind`, then `npx mastermind init` | Node.js 24+ |
| Cargo | `cargo install mmcg --locked`, then `mmcg init --workflow off` | Rust 1.96+ |

Cargo supplies the native CLI without the npm workflow bundle. Sources:
[npm manifest](../npm/mastermind/package.json),
[Cargo manifest](../mcp/servers/mmcg/Cargo.toml).

### Current source build

From this repository's source checkout:

```bash
cargo install --path mcp/servers/mmcg --locked
```

Then run inside the repository you want to work on:

```bash
mmcg init --workflow off
```

This uses the current native CLI without the npm workflow installer. Native
client setup, profile access and task mining are available on macOS/Linux.

For component-level setup, use `mastermind install`, `mastermind setup`, or
`mastermind miner hooks`. Client guides cover advanced scope and removal:
[Claude Code](integrations/claude-code.md), [Codex](integrations/codex.md),
[Cursor](integrations/cursor.md), [Continue](integrations/continue.md), and
[generic MCP](integrations/generic-mcp.md).

## Storage and provider access

| Location | Contents |
|---|---|
| `.mastermind/setup.json` | Project-bound setup choices, not permission or runtime proof |
| `.mastermind/mmcg.db` | Code and documentation index and scratchpad |
| `.mastermind/tasks/` | Specs, checks, execution, audit, and review records |
| `CONTEXT.md` | Maintained project knowledge |
| `~/.mastermind/` | Optional personal profile, capture journal, and worker records |
| Client configuration | MCP registration and workflow adapters |

`init` creates `.mastermind/.gitignore` for local working data. Preserve that
ignore policy when sharing project files.

| Operation | Model access |
|---|---|
| Default scaffold, indexing, deterministic queries, Lens, export | None |
| Task mining | Current agent, no separate request |
| `init --draft-with claude` | Explicit Claude-assisted document drafting |
| Mining or refinement with an explicit provider | Captured episodes or user prompts sent through the selected provider |
| Native task execution and review | Explicit Claude operations |

Use `mastermind --help` for the short command catalog and `mmcg --help` for the
full native catalog.
