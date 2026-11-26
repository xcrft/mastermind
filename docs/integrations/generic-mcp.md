# Other MCP clients

This guide connects Cursor, Continue or any client that can launch a stdio MCP
server to the `mmcg` server. For Claude Code and Codex, `mastermind init` does
it, see [getting started](../getting-started.md).

## Register the server

1. Install Mastermind and index the repository:

   ```bash
   npm install -g @xcraftmind/mastermind
   cd your-project
   mastermind index .
   ```

2. Preview the change, then apply it with `--write`:

   ```bash
   mastermind setup cursor --scope project --root .
   mastermind setup cursor --scope project --root . --write
   ```

   | Client | `setup` target | Configuration file, project scope or `--scope user` |
   |---|---|---|
   | Cursor | `cursor` | `.cursor/mcp.json` or `~/.cursor/mcp.json` |
   | Continue | `continue` | `.continue/mcpServers/mastermind.yaml` or `~/.continue/mcpServers/mastermind.yaml` |
   | Any client with JSON `mcpServers` | `generic --config ./mcp.json` | The file you pass to `--config` |

   Without `--scope`, setup uses user scope. It merges into the file, keeps
   unrelated entries, owns a separate `mastermind.yaml` for Continue and picks
   the launcher command for your installation, Windows npm included.

3. Restart or reload the client, then check the registration and the project:

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

A cargo install provides only `mmcg` as the command. To point the server at a
specific index, put `--index /absolute/path/project/.mastermind/mmcg.db` before
`serve` in `args`. Don't use that form in a Claude Code `.mcp.json`, because the
[workflow audit](../reference/mmcg.md#workflow-audit) expects exactly
`["serve"]` there.

## Pick a tool

| Need | Start with |
|---|---|
| Briefing for a role | `mmcg_brief` |
| Reasoning effort for a new agent | `mmcg_effort_route`, see [effort routing](../guides/effort-routing.md) |
| A name or a concept | `mmcg_search`, `mmcg_concept` |
| Whether a symbol does an operation | `mmcg_concept_prepare`, source reads, `mmcg_concept_finish` |
| Structure or change impact | `mmcg_map`, `mmcg_change_impact` |
| Project documentation | `mmcg_docs`, `mmcg_project_profile` |
| Combined context | `mmcg_context` |
| Task records | `mmcg_tasks`, `mmcg_history` |
| Granted personal preferences | `mmcg_profile` |

`tools/list` returns the names and schemas, and the
[MCP tool reference](../reference/mcp.md#mcp-tools) has the arguments and
limits. The server speaks JSON-RPC over stdin and stdout and lists tools only,
with no resources or prompts, and the supported MCP versions and message limits
are in the [protocol contract](../reference/mcp.md#protocol-contract). Graph
queries may refresh the derived index, and `mmcg_mining_submit` is the only
other local write.

## Give the client your profile

Personal context needs a read grant for this repository and a client ID that
the server sends with each request:

```bash
mastermind miner access grant . --client my-client
mastermind setup cursor --scope project --root . --profile-client my-client --write
```

`--profile-client` writes `MMCG_PROFILE_CLIENT=my-client` into the server
entry, and for a hand-written entry you set that variable in its `env`. Without
a grant the server withholds personal context, with no fallback to a static
`style.md`.

## Remove the entry

Run the same `setup` command with `--remove` and the scope you registered with,
check the preview, then add `--write`. A customized entry also needs `--force`,
and a forced change saves a backup under `~/.mastermind/setup-backups/`.
