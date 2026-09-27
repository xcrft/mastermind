# Claude Code integration

Connect the local graph through MCP. Install the workflow bundle for skills
and Claude's named subagents.

## Install the workflow

```bash
npm install -g @xcraftmind/mastermind
mastermind install --client claude
cd your-project
mastermind index .
mastermind doctor --workflow --client claude
```

| Option | Result |
|---|---|
| Default | `core` skills |
| `--profile frontend`, `security`, `full` | Expanded skill selection |
| Restart Claude Code | Load the installed workflow |

## Register MCP only

| Scope | Storage |
|---|---|
| User | Native `claude mcp` registration |
| Project | Repository `.mcp.json` |

```bash
mastermind setup claude --scope user
mastermind setup claude --scope user --write
```

For project scope:

```bash
mastermind setup claude --scope project --root .
mastermind setup claude --scope project --root . --write
```

The command without `--write` previews the change. Setup preserves unrelated
entries. MCP does not require `mastermind init`.

## Verify or remove

```bash
mastermind doctor
mastermind setup claude --scope project --root . --remove
mastermind setup claude --scope project --root . --remove --write
```

| Condition | Behavior |
|---|---|
| Removal | Use the installation scope |
| Matching registration | No change needed |
| Customized entry | Replacement requires `--force` and `--write` |
| Forced file change | Backup under `~/.mastermind/setup-backups/` |
| Doctor | Reads configuration and reports project health |

## Use it

[Workflow](../workflow.md) covers handoff and `run-task --exec` with existing
Claude authentication and permissions. [Persona hooks](../guides/persona-hooks.md)
separately enables capture and profile delivery.
