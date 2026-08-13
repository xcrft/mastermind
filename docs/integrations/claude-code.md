# Claude Code integration

Claude Code supports project-local `.mcp.json` configuration and user-scope
registration through `claude mcp`.

## User scope

Install Mastermind, index the repository, and preview registration:

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

User scope uses the bounded native `claude mcp` contract. Mastermind compares
the exact trimmed `Command:` and `Args:` fields, rejects truncated inspection
output, and rechecks executable identity before later inspection or mutation.
The process runs without a shell, has a ten-second limit, and does not print raw
output.

## Project scope

```bash
npm install -D @xcraftmind/mastermind
mastermind setup claude --scope project --root .          # dry-run
mastermind setup claude --scope project --root . --write
```

Project scope merges the canonical `mmcg` entry into `.mcp.json` while
preserving unrelated root fields and servers. The legacy spelling
`--project . --write-mcp` remains compatible.

## Update or remove

A matching entry is an idempotent no-op. A customized entry requires `--force`;
`--force` never implies `--write`. Before forced file-backed replacement or
removal, Mastermind stores the previous bytes under
`~/.mastermind/setup-backups/`.

```bash
mastermind setup claude --scope project --root . --remove          # dry-run
mastermind setup claude --scope project --root . --remove --write
```

## Verify

```bash
mastermind doctor
```

Doctor parses supported configuration locations as bounded data, rejects
symlinked config files or existing path ancestors, and reports only client
labels and structural status. It does not execute commands from configuration;
the separate MCP handshake starts only the trusted current Mastermind binary.

Restart Claude Code after changing registration.
