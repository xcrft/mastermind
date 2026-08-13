# Codex CLI integration

Mastermind registers with Codex at user scope through `codex mcp`. Project
scope is not supported.

## Install and register

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

Codex receives the portable planning, execution, project-map, impact, test,
setup, review, and audit skills. It does not receive Claude Code's native
spawnable subagent files because Codex has a different agent runtime. This is
workflow-contract compatibility, not a claim that both clients expose identical
role orchestration.

The normal task handoff is client-neutral:

```bash
mastermind verify-spec .mastermind/tasks/001-example/spec.md
# approve Scope and Acceptance Criteria
mastermind run-task .mastermind/tasks/001-example/spec.md --pre-only
# ask Codex to implement the spec and write executor-report.md
mastermind run-task .mastermind/tasks/001-example/spec.md --post-only
```

Avoid `run-task --exec` in Codex workflows; that compatibility flag invokes the
Claude CLI specifically.

Apply the native registration explicitly:

```bash
mastermind setup codex --scope user --write
```

`mastermind setup codex --scope project` is rejected before configuration reads
or subprocesses. Mastermind resolves Codex only from absolute `PATH` entries
outside the current repository and invokes it without a shell. JSON inspection
must contain the exact stdio command and ordered arguments. Truncated output is
rejected, and executable identity is rechecked before every inspection or
mutation within the ten-second bound.

## Update or remove

A matching native entry is an idempotent no-op. A customized entry requires
`--force`; `--force` never implies `--write`.

```bash
mastermind setup codex --scope user --remove          # dry-run
mastermind setup codex --scope user --remove --write
```

## Verify

```bash
mastermind doctor
mastermind doctor --workflow --client codex
```

Doctor recognizes only the `[mcp_servers.mmcg]` table with a string `command`
and ordered string `args` in `~/.codex/config.toml`. It parses normal TOML
quoting, comments, and multiline arrays, while rejecting malformed types,
duplicate keys, and symlinked config ancestry. Doctor does not execute Codex or
a configured command. The old YAML configuration shape is unsupported.
