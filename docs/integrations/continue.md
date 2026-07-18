# Continue integration

Continue setup uses one Mastermind-owned YAML document at `.continue/mcpServers/mastermind.yaml` for project scope or `~/.continue/mcpServers/mastermind.yaml` for user scope.

## Setup

```text
mastermind setup <claude|cursor|codex|continue|generic> \
  --scope <project|user> [--root .] [--config PATH] [--write] [--remove] [--force]
```

Install and index Mastermind, then preview the selected owned file:

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

The owned document contains only `schema: 1`, `owner: mastermind`, `name: mmcg`, `command`, and `args`. Mastermind does not merge this entry into Continue's general JSON configuration.

## Updating and removing

A canonical owned document is an idempotent no-op and can be removed safely. Customized content is refused unless `--force` is supplied; `--force` never implies `--write`. Before a forced change, the old bytes are backed up privately under `~/.mastermind/setup-backups/`.

```bash
mastermind setup continue --scope project --root . --remove          # dry-run
mastermind setup continue --scope project --root . --remove --write
```

## Verification

```bash
mastermind doctor
```

Doctor parses the owned YAML as bounded data, rejects symlinked files or existing path ancestors, compares the document structurally with the trusted current entry, and never executes its configured command. Reload Continue after applying a change. The former experimental JSON instructions are not supported.
