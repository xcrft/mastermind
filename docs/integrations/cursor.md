# Cursor integration

Cursor supports project MCP configuration at `.cursor/mcp.json` and user configuration at `~/.cursor/mcp.json`.

## Setup

```text
mastermind setup <claude|cursor|codex|continue|generic> \
  --scope <project|user> [--root .] [--config PATH] [--write] [--remove] [--force]
```

Install and index Mastermind, then preview the selected target:

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

The setup engine preserves unrelated root fields and MCP servers, rejects duplicate JSON keys and unsafe paths, and does not write without `--write`.

## Updating and removing

A canonical entry is an idempotent no-op. Customized replacement or removal requires `--force`, which does not imply `--write`. Forced file-backed changes save the previous bytes privately under `~/.mastermind/setup-backups/`.

```bash
mastermind setup cursor --scope project --root . --remove          # dry-run
mastermind setup cursor --scope project --root . --remove --write
```

## Verification

```bash
mastermind doctor
```

Doctor treats Cursor configuration as bounded data, reports only structural status, and never executes the configured command. Restart Cursor after applying a change. Keep `.mastermind/` out of version control.
