# Continue integration

Mastermind owns a standalone MCP document for Continue.

## Register

```bash
npm install -g @xcraftmind/mastermind
cd your-project
mastermind index .
mastermind setup continue --scope project --root .
mastermind setup continue --scope project --root . --write
```

| Scope | Configuration |
|---|---|
| Project | `.continue/mcpServers/mastermind.yaml` |
| User | `~/.continue/mcpServers/mastermind.yaml` |

For user scope, replace `--scope project --root .` with `--scope user`.
Setup previews without `--write` and selects the launcher for your installation,
including Windows npm. It does not merge into Continue's general configuration.

Reload Continue, then inspect registration and project health:

```bash
mastermind doctor
```

## Change or remove

```bash
mastermind setup continue --scope project --root . --remove
mastermind setup continue --scope project --root . --remove --write
```

| Condition | Action |
|---|---|
| Removal | Select the original scope |
| Customized content | Add `--force` and `--write` |
| Forced file change | Backup at `~/.mastermind/setup-backups/` |

See [Workflow](../workflow.md) and the
[MCP reference](../reference/mmcg.md#mcp-tools).
