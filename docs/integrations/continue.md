# Continue integration

Mastermind owns one standalone Continue YAML document:

| Scope | Path |
|---|---|
| Project | `.continue/mcpServers/mastermind.yaml` |
| User | `~/.continue/mcpServers/mastermind.yaml` |

## Preview and apply

```bash
npm install -g @xcraftmind/mastermind
cd your-project
mastermind index .
mastermind setup continue --scope project --root .
```

Apply project or user registration explicitly:

```bash
mastermind setup continue --scope project --root . --write
mastermind setup continue --scope user --write
```

Mastermind writes Continue's standalone MCP schema. For a global npm install, the generated document is equivalent to:

```yaml
name: Mastermind MCP
version: 1.0.0
schema: v1
mcpServers:
  - name: mmcg
    command: mastermind
    args:
      - serve
```

The `command` and `args` follow the detected install mode. On Windows, global,
npx, and project-local npm launchers use `cmd.exe /d /s /c` so Continue can
execute npm's `.cmd` shims. Mastermind does not merge this entry into Continue's
general JSON configuration.

## Update or remove

A canonical owned document is an idempotent no-op and can be removed safely.
Customized content requires `--force`; `--force` never implies `--write`.
Before a forced change, Mastermind stores the previous bytes under
`~/.mastermind/setup-backups/`.

```bash
mastermind setup continue --scope project --root . --remove          # dry-run
mastermind setup continue --scope project --root . --remove --write
```

## Verify

```bash
mastermind doctor
```

Doctor parses the owned YAML as bounded data, rejects symlinked files or
existing path ancestors, and compares the full document with the trusted
current entry. It does not execute the configured command. Reload Continue
after applying a change. The former experimental JSON shape is unsupported.
