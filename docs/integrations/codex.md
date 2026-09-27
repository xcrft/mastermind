# Codex integration

Install portable skills and connect the local graph through user-scope MCP.

## Install

```bash
npm install -g @xcraftmind/mastermind
cd your-project
mastermind init --client codex
mastermind status
mastermind doctor --workflow --client codex
```

| Setting | Behavior |
|---|---|
| Fresh install | `core` skills |
| `mastermind update --workflow-only --client codex --profile frontend` | Select frontend skills. Other profiles are `security` and `full` |
| Update without `--profile` | Keep the selected profile |
| Agent runtime | Codex uses portable skills, not Claude-native subagent files |

`init` indexes code and documents and configures local capture. Restart Codex,
trust the project and inspect `/hooks`. Semantic mining requires
`--mining on --provider claude`. On Windows select `--mining off` because native
hooks and managed workers require Unix. See [onboarding](../getting-started.md).

## Register MCP only

```bash
mastermind setup codex --scope user
mastermind setup codex --scope user --write
```

Setup previews first, uses `codex mcp` to apply, then checks the enabled `mmcg`
entry. Project-scope setup is unsupported. Restart Codex afterward.

## Use the task workflow

| Step | Owner |
|---|---|
| Approve spec and run preflight | Planner and controller |
| Implement and write executor report | Codex |
| Run declared checks and postflight | Runner and controller |
| Assess evidence and resolve history | Reviewer |
| Complete | Controller |

See [Workflow](../workflow.md) for commands.
`run-task --exec` and `review-task run` launch Claude Code, even when the
implementation was done in Codex.

## Verify or remove

```bash
mastermind doctor
mastermind setup codex --scope user --remove
mastermind setup codex --scope user --remove --write
```

| Condition | Behavior |
|---|---|
| Matching enabled entry | No change |
| Customized or disabled entry | Requires `--force` and `--write` |
| Native registration changed | Setup checks the resulting entry |
| Doctor | Reads `[mcp_servers.mmcg]` in `~/.codex/config.toml` |

Registration checks and server handshake are distinct.
[Persona hooks](../guides/persona-hooks.md) requires separate capture and
global-profile read grants.
