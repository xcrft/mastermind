# Codex CLI integration

Give Codex the bounded local graph plus Mastermind's portable delivery skills.
Mastermind registers through `codex mcp` at user scope; Codex does not currently
have a project-scope setup path.

## Install the Codex workflow

```bash
npm install -g @xcraftmind/mastermind
mastermind install --client codex
cd your-project
mastermind index .
```

The install command defaults to the `core` portable skill profile under
`~/.codex/skills` and registers the MCP server. Pass `--profile full` for every
shipped portable skill. It does not require `mastermind init` or a repository.
To preview MCP registration without installing skills, use `mastermind setup
codex --scope user`.

Codex receives the portable planning, execution, project-map, impact, test,
setup, review, and audit skills. It does not receive Claude Code's native
spawnable subagent files because Codex has a different agent runtime. This is
workflow-contract compatibility, not a claim that both clients expose identical
role orchestration.

Once installed, the normal task handoff stays client-neutral:

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

## Change or undo safely

A matching native entry is an idempotent no-op. A customized entry requires
`--force`; `--force` never implies `--write`.

```bash
mastermind setup codex --scope user --remove          # dry-run
mastermind setup codex --scope user --remove --write
```

## Verify the result

```bash
mastermind doctor
mastermind doctor --workflow --client codex
```

Doctor recognizes only the `[mcp_servers.mmcg]` table with a string `command`
and ordered string `args` in `~/.codex/config.toml`. It parses normal TOML
quoting, comments, and multiline arrays, while rejecting malformed types,
duplicate keys, and symlinked config ancestry. Doctor does not execute Codex or
a configured command. The old YAML configuration shape is unsupported.
