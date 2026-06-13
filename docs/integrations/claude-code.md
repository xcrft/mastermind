# Claude Code integration

Mastermind's primary integration. The `mastermind setup claude` command handles registration automatically.

## Quick setup

```bash
npm install -g @xcraftmind/mastermind
cd your-project
mastermind init
mastermind setup claude --write-mcp
```

Restart Claude Code. The codegraph tools are now available in every session.

## What `setup claude` does

Runs `claude mcp add --scope user mmcg -- mastermind serve`, which writes an entry to `~/.claude.json`:

```json
{
  "mcpServers": {
    "mmcg": {
      "command": "mastermind",
      "args": ["serve"]
    }
  }
}
```

The `--scope user` flag makes the server available across all projects without per-project config.

## Project-local registration

To pin the version per project instead of using the global install:

```bash
npm install -D @xcraftmind/mastermind
mastermind setup claude --project . --write-mcp
```

This writes `.mcp.json` in the project root with `command: "./node_modules/.bin/mastermind"`. Claude Code picks up `.mcp.json` automatically when the project is open.

## Verifying the connection

```bash
mastermind doctor
```

The `serve handshake` check confirms Claude Code can start and query the MCP server. All checks should be green before starting a workflow session.

## Workflow subagents

`mastermind init` installs the workflow subagents and skills into `~/.claude/agents/` and `~/.claude/skills/`. These are the planning, execution, auditing, and critique roles described in the workflow CLAUDE.md template.

To install subagents without re-running init:

```bash
mastermind setup claude --with-workflow --write-mcp
```

## Troubleshooting

**`mmcg` not found after `setup claude`** — ensure `mastermind` is on PATH (`which mastermind`). If installed globally via npm, check that npm's bin directory is in PATH.

**MCP server not showing in Claude Code** — restart Claude Code after running `setup claude`. The server list is read on startup.

**`mastermind doctor` fails on `serve handshake`** — run `mastermind serve` manually and check for startup errors. Common causes: index file missing (run `mastermind index .`) or stale binary path after reinstall.

**Re-registering after reinstall** — use `--force` to overwrite an existing entry:

```bash
mastermind setup claude --write-mcp --force
```
