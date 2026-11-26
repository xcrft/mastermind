# Codex integration

`mastermind init --client codex` connects Codex to the local graph through
user-scope MCP and installs the portable skills and the workflow block in
`AGENTS.md`. Codex uses skills, not the Claude subagent files.

## Install

```bash
npm install -g @xcraftmind/mastermind
cd your-project
mastermind init --client codex
mastermind status
mastermind doctor --workflow --client codex
```

`doctor --workflow` comes from the npm package. What else `init` does on the
first run is in [getting started](../getting-started.md).

Restart Codex, trust the project and look at `/hooks`. Native hooks need macOS
or Linux, so on Windows run `init` with `--mining off`.

A fresh install selects the `core` skills. To switch the bundle:

```bash
mastermind update --workflow-only --client codex --profile frontend
```

The other profiles are `security` and `full`, and an update without `--profile`
keeps the installed one.

## Register MCP only

```bash
mastermind setup codex --scope user
mastermind setup codex --scope user --write
```

The first command previews. The second applies the change through `codex mcp`
and then checks the enabled `mmcg` entry. Codex has no project-scope setup, so
`--scope project` is refused. Restart Codex afterwards.

## Use the task workflow

| Step | Who does it |
|---|---|
| Approve the spec and run preflight | Planner and controller |
| Implement and write the executor report | Codex |
| Run the declared checks and postflight | Check runner and controller |
| Assess the evidence and decide on project knowledge | Reviewer |
| Complete | Controller |

The [workflow guide](../workflow.md) has the commands. `run-task --exec` and
`review-task run` launch Claude Code, even when Codex did the implementation.

## Route reasoning effort

The core workflow includes the `mastermind-agent-handoff` skill. Planning,
research and delegated execution use it before each new agent:

1. Send the receiving agent's duty, family, source scope and risk to `mmcg_effort_route` or `mastermind effort-route`.
2. Merge the returned `codex_spawn_args` into the normal `spawn_agent` call, and pass the bounded role and task packet to the new child.
3. Keep the decision and the child ID, then check the child's own `turn_context.effort`.

A full-history fork keeps the parent's effort and is recorded as `not_applied`.
The selected model and permissions stay as they are.
[Effort routing](../guides/effort-routing.md) has the inputs, launch commands
and fallbacks.

## Check or remove

```bash
mastermind doctor
mastermind setup codex --scope user --remove
mastermind setup codex --scope user --remove --write
```

| Condition | What to do |
|---|---|
| Entry matches and is enabled | Nothing |
| Customized or disabled entry | Add `--force` with `--write` |
| `doctor` reports a problem | It reads `[mcp_servers.mmcg]` in `~/.codex/config.toml`. A registered entry still needs a working server handshake |
| Another dispatcher such as TokenX rejects explicit `reasoning_effort` | Pick one effort authority. The handoff skill stops before spawn until you do. To use Mastermind, disable that dispatcher's non-managed hooks in `/hooks` and start a new session, see [Codex hook controls](https://developers.openai.com/codex/hooks#review-and-trust-hooks) |

`init` also sets up capture and profile read grants for this client and
project. [Persona hooks](../guides/persona-hooks.md) covers source inspection
and review.
