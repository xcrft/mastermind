# Cursor integration

Connect Cursor to the local graph while preserving every unrelated MCP server
and root setting. Use project scope for `.cursor/mcp.json` or user scope for
`~/.cursor/mcp.json`.

## Preview first, then apply

```bash
npm install -g @xcraftmind/mastermind
cd your-project
mastermind index .
mastermind setup cursor --scope project --root .
```

Apply project or user registration explicitly:

```bash
mastermind setup cursor --scope project --root . --write
mastermind setup cursor --scope user --write
```

The setup engine preserves unrelated root fields and MCP servers, rejects
duplicate JSON keys and unsafe paths, and does not write without `--write`.

## Change or undo safely

A canonical entry is an idempotent no-op. Customized replacement or removal
requires `--force`, which does not imply `--write`. Forced file-backed changes
save the previous bytes under `~/.mastermind/setup-backups/`.

```bash
mastermind setup cursor --scope project --root . --remove          # dry-run
mastermind setup cursor --scope project --root . --remove --write
```

## Verify the result

```bash
mastermind doctor
```

Doctor treats Cursor configuration as bounded data, reports only structural
status, and does not execute the configured command. Restart Cursor after
applying a change. Keep `.mastermind/` out of version control.
