# Claude Code integration

`mastermind init --client claude` connects Claude Code to the local graph over
MCP and installs the workflow bundle: skills, named subagents and the workflow
block in `CLAUDE.md`. Claude Code is also the client that runs native task
execution and review.

## Install the workflow

```bash
npm install -g @xcraftmind/mastermind
cd your-project
mastermind init --client claude
mastermind status
mastermind doctor --workflow --client claude
```

`doctor --workflow` checks the installed workflow files against the package.
It comes from the npm package, so a cargo-only install doesn't have it.
What else `init` does on the first run is in
[getting started](../getting-started.md).

Restart Claude Code and review the hook definitions in `/hooks`. Native hooks
need macOS or Linux, so on Windows run `init` with `--mining off`.

A fresh install selects the `core` skills. To switch the bundle:

```bash
mastermind update --workflow-only --client claude --profile frontend
```

The other profiles are `security` and `full`, and an update without `--profile`
keeps the installed one.

## Register MCP only

MCP works without `init`. Setup previews the change, and `--write` applies it
while keeping unrelated entries.

| Scope | Where it goes | Commands |
|---|---|---|
| User | Native `claude mcp` registration | `mastermind setup claude --scope user`, then the same with `--write` |
| Project | The repository's `.mcp.json` | `mastermind setup claude --scope project --root .`, then the same with `--write` |

## Check or remove

```bash
mastermind doctor
mastermind setup claude --scope project --root . --remove
mastermind setup claude --scope project --root . --remove --write
```

`doctor` reads the configuration and reports project health. Remove with the
scope you installed with. A registration that already matches needs no change,
and a customized entry needs `--force` with `--write`. A forced change saves a
backup under `~/.mastermind/setup-backups/`.

## Use it

The [workflow guide](../workflow.md) covers handoff and `run-task --exec`,
which uses your existing Claude authentication and permissions.
[Persona hooks](../guides/persona-hooks.md) covers source inspection, candidate
review and the optional separate miner.

## Route reasoning effort

The core workflow includes the `mastermind-agent-handoff` skill. Before it
starts a new agent, it asks `mmcg_effort_route` or `mastermind effort-route` for
an effort level and applies it through the control the client has:

| Invocation | Control |
|---|---|
| New CLI session | `mastermind effort-route --request route.json --exec claude -- --print ...` passes `claude_args` |
| Non-fork `Agent`, Claude Code 2.1.292 or later | Pass `effort: requested_effort`. `CLAUDE_CODE_EFFORT_LEVEL` takes precedence |
| Older client or another path | Keep the agent defaults and record `not_applied` |

The selected model and permissions stay as they are, and sessions or subagents
that are already running keep their settings. [Effort routing](../guides/effort-routing.md)
has the request contract, checks and recovery.
