# Codex CLI integration

Codex setup is supported at user scope through the native `codex mcp` command. Project scope is intentionally unsupported.

## Setup

```text
mastermind setup <claude|cursor|codex|continue|generic> \
  --scope <project|user> [--root .] [--config PATH] [--write] [--remove] [--force]
```

Install and index Mastermind, then preview registration:

```bash
npm install -g @xcraftmind/mastermind
mastermind install --client codex
cd your-project
mastermind index .
```

The install command copies the portable skill bundle under `~/.codex/skills`
and registers the MCP server. It does not require `mastermind init` or a
repository. To preview MCP registration without installing skills, use
`mastermind setup codex --scope user`.

Apply the native registration explicitly:

```bash
mastermind setup codex --scope user --write
```

`mastermind setup codex --scope project` is rejected before configuration reads or subprocesses. Codex is resolved only from absolute `PATH` entries outside the current repository and invoked without a shell. Its JSON inspection must contain the exact stdio command and ordered arguments, truncated output fails closed, and executable identity is rechecked before every inspect or mutation within the ten-second bound.

## Updating and removing

A matching native entry is an idempotent no-op. A customized entry requires `--force`; `--force` never implies `--write`.

```bash
mastermind setup codex --scope user --remove          # dry-run
mastermind setup codex --scope user --remove --write
```

## Verification

```bash
mastermind doctor
mastermind doctor --workflow --client codex
```

Doctor recognizes only the `[mcp_servers.mmcg]` table with a string `command` and ordered string `args` in `~/.codex/config.toml`. It parses the file as TOML, including normal quoting, comments, and multiline arrays, while rejecting malformed types, duplicate keys, and symlinked config ancestry. Doctor never executes Codex or a configured command. The old YAML configuration instructions are not supported.
