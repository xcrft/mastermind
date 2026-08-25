# Claude Code integration

Give Claude Code the local graph without hand-editing its MCP configuration.
Use user scope for one installation across repositories or project scope when
the repository should carry its own `.mcp.json` entry.

## Fastest path: user scope

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

## Repository-owned path: project scope

```bash
npm install -D @xcraftmind/mastermind
mastermind setup claude --scope project --root .          # dry-run
mastermind setup claude --scope project --root . --write
```

Project scope merges the canonical `mmcg` entry into `.mcp.json` while
preserving unrelated root fields and servers. The legacy spelling
`--project . --write-mcp` remains compatible.

## Change or undo safely

A matching entry is an idempotent no-op. A customized entry requires `--force`;
`--force` never implies `--write`. Before forced file-backed replacement or
removal, Mastermind stores the previous bytes under
`~/.mastermind/setup-backups/`.

```bash
mastermind setup claude --scope project --root . --remove          # dry-run
mastermind setup claude --scope project --root . --remove --write
```

## Verify the result

```bash
mastermind doctor
```

Doctor parses supported configuration locations as bounded data, rejects
symlinked config files or existing path ancestors, and reports only client
labels and structural status. It does not execute commands from configuration;
the separate MCP handshake starts only the trusted current Mastermind binary.
For installed `mastermind-*` agents it also verifies the full runtime chain:
explicit model, tools, `maxTurns`, and effort; registered MCP servers; exact
known mmcg grants; and every mmcg tool named by the prompt.

Restart Claude Code after changing registration.
