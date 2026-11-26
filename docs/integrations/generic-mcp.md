# Other MCP clients

This guide connects Cursor, Continue or any client that can launch a stdio MCP
server to the `mmcg` server. Claude Code and Codex have their own guides,
[Claude Code](claude-code.md) and [Codex](codex.md), because `mastermind init`
sets them up for you.

Install Mastermind and index the repository first:

```bash
npm install -g @xcraftmind/mastermind
cd your-project
mastermind index .
```

## Register the server

Each `setup` command previews the change first. Add `--write` to apply it.
Setup merges into the file and keeps unrelated entries.

| Client | Commands | Configuration file |
|---|---|---|
| Cursor | `mastermind setup cursor --scope project --root .` | `.cursor/mcp.json`, or `~/.cursor/mcp.json` with `--scope user` |
| Continue | `mastermind setup continue --scope project --root .` | `.continue/mcpServers/mastermind.yaml`, or `~/.continue/mcpServers/mastermind.yaml` with `--scope user` |
| Any client with JSON `mcpServers` | `mastermind setup generic --scope project --config ./mcp.json` | The file you pass to `--config` |

For example, for Cursor:

```bash
mastermind setup cursor --scope project --root .
mastermind setup cursor --scope project --root . --write
```

Without `--scope`, setup uses user scope. For Continue, Mastermind owns a
separate `mastermind.yaml` and doesn't touch Continue's general configuration.
Setup picks the launcher command for your installation, Windows npm included.

Restart or reload the client, then check the registration and the project:

```bash
mastermind doctor
```

To write the entry by hand, use this JSON and start the client from the
repository root:

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

The npm package provides both `mastermind` and `mmcg` as the command. A cargo
install provides only `mmcg`. To point the server at a specific index, put
`--index /absolute/path/project/.mastermind/mmcg.db` before `serve` in `args`.
Don't use that form in a Claude Code `.mcp.json`, because the
[workflow audit](../reference/mmcg.md#workflow-audit) expects exactly
`["serve"]` there.

## Pick a tool

| Need | Start with |
|---|---|
| Briefing for a role | `mmcg_brief` |
| Reasoning effort for a task or agent | `mmcg_effort_route` |
| A name or a concept | `mmcg_search`, `mmcg_concept` |
| Whether a symbol does an operation | `mmcg_concept_prepare`, source reads, `mmcg_concept_finish` |
| Structure or change impact | `mmcg_map`, `mmcg_change_impact` |
| Project documentation | `mmcg_docs`, `mmcg_project_profile` |
| Combined context | `mmcg_context` |
| Task records | `mmcg_tasks`, `mmcg_history` |
| Granted personal preferences | `mmcg_profile` |

`tools/list` returns the names and schemas, and the
[MCP tool reference](../reference/mcp.md#mcp-tools) has the arguments and
limits. Concept review is described in
[concept search](../reference/concept-search.md#review-operation-relevance).
Graph queries may refresh the derived index, and `mmcg_mining_submit` is the
only other local write.

`mmcg_effort_route` returns a requested level and client argument arrays. It
doesn't change the current chat or launch a model, so apply it through a
per-agent control before the next invocation, see
[effort routing](../guides/effort-routing.md).

## Give the client your profile

Personal context needs a read grant for this repository and a client ID that
the server sends with each request:

```bash
mastermind miner access grant . --client my-client
mastermind setup cursor --scope project --root . --profile-client my-client --write
```

`--profile-client` writes `MMCG_PROFILE_CLIENT=my-client` into the server entry.
For a hand-written entry, set that variable in its `env`. Without a grant the
server withholds personal context, and there is no fallback to a static
`style.md`. [Persona hooks](../guides/persona-hooks.md) covers capture and
review.

## Protocol

The server speaks JSON-RPC over stdin and stdout and lists tools only, with no
resources or prompts. Supported MCP versions and message limits are in the
[protocol contract](../reference/mcp.md#protocol-contract).

## Remove the entry

Run the same `setup` command with `--remove`, check the preview, then add
`--write`. Use the scope you registered with.

```bash
mastermind setup cursor --scope project --root . --remove
mastermind setup cursor --scope project --root . --remove --write
```

An entry that matches what setup would write needs no change. A customized
entry also needs `--force`, and a forced change saves a backup under
`~/.mastermind/setup-backups/`.
