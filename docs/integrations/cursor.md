# Cursor integration

## Register

```bash
npm install -g @xcraftmind/mastermind
cd your-project
mastermind index .
mastermind setup cursor --scope project --root .
mastermind setup cursor --scope project --root . --write
```

| Scope | Configuration |
|---|---|
| `project` | `.cursor/mcp.json` |
| `user` | `~/.cursor/mcp.json` |

Setup previews without `--write` and preserves unrelated entries.
Reload Cursor, then inspect registration and project health:

```bash
mastermind doctor
```

## Change or remove

```bash
mastermind setup cursor --scope project --root . --remove
mastermind setup cursor --scope project --root . --remove --write
```

| Condition | Action |
|---|---|
| Removal | Select the original scope |
| Matching entry | No change |
| Customized entry | Add `--force` and `--write` |
| Forced file change | Backup at `~/.mastermind/setup-backups/` |

Use [Workflow](../workflow.md) for controlled tasks and the
[MCP reference](../reference/mmcg.md#mcp-tools) for tool arguments.
