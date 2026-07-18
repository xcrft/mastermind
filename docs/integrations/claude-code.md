# Claude Code integration

Claude Code supports project-local JSON and user-scope registration through its native MCP CLI.

## Setup

```text
mastermind setup <claude|cursor|codex|continue|generic> \
  --scope <project|user> [--root .] [--config PATH] [--write] [--remove] [--force]
```

Install and index Mastermind, then preview user registration:

```bash
npm install -g @xcraftmind/mastermind
cd your-project
mastermind init
mastermind setup claude --scope user
```

The preview resolves the trusted current Mastermind command and shows only a redacted command summary. Apply it with:

```bash
mastermind setup claude --scope user --write
```

User scope uses the bounded native `claude mcp` contract. Mastermind compares the exact trimmed `Command:` and `Args:` fields, fails closed if inspection output is truncated, and rejects a changed executable identity before any later inspect or mutation. The process is invoked without a shell, limited to ten seconds, and its raw output is never printed.

## Project-local registration

```bash
npm install -D @xcraftmind/mastermind
mastermind setup claude --scope project --root .          # dry-run
mastermind setup claude --scope project --root . --write
```

Project scope safely merges the canonical `mmcg` entry into `.mcp.json` while preserving unrelated root fields and servers. The legacy spelling `--project . --write-mcp` remains compatible.

## Updating and removing

A matching entry is an idempotent no-op. A customized entry is refused unless `--force` is supplied; `--force` never implies `--write`. Before forced file-backed replacement or removal, the old bytes are stored privately under `~/.mastermind/setup-backups/`.

```bash
mastermind setup claude --scope project --root . --remove          # dry-run
mastermind setup claude --scope project --root . --remove --write
```

## Verification

```bash
mastermind doctor
```

Doctor parses supported configuration locations as bounded data, rejects symlinked config files or existing path ancestors, reports only client labels and structural statuses, and never executes commands read from configuration. Its separate MCP handshake starts only the trusted current Mastermind binary.

Restart Claude Code after changing registration.
