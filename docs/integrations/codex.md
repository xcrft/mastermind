# Codex CLI integration

[OpenAI Codex CLI](https://github.com/openai/codex) supports MCP servers via `~/.codex/config.yaml`.

## Setup

**1. Install Mastermind**

```bash
npm install -g @xcraftmind/mastermind
```

**2. Index your project**

```bash
cd your-project
mastermind index .
```

**3. Add mmcg to Codex config**

Edit `~/.codex/config.yaml`:

```yaml
mcp_servers:
  - name: mmcg
    command: mastermind
    args:
      - serve
```

**4. Verify**

```bash
codex --list-tools
```

The 20 `mmcg_*` tools should appear in the output.

## Project-specific index

If you work across multiple projects and want Codex to use the correct index, pass the index path explicitly:

```yaml
mcp_servers:
  - name: mmcg
    command: mastermind
    args:
      - --index
      - /path/to/your-project/.mastermind/mmcg.db
      - serve
```

Alternatively, launch Codex from the project root — `mastermind serve` resolves `.mastermind/mmcg.db` relative to the current working directory.

## Using mmcg in Codex sessions

Once connected, reference mmcg tools in your Codex prompts:

```
Who calls parseConfig? Use mmcg_callers.
What's the blast radius of changing AuthService? Use mmcg_impact.
List dead-code candidates in the auth/ prefix. Use mmcg_unreferenced.
```

Codex will invoke the tools directly and include the graph results in its context.

## Notes

- The Mastermind workflow subagents (planner, auditor, critic, etc.) are Claude Code-specific.
- mmcg tools work with any model Codex is configured to use.
- Add `.mastermind/` to `.gitignore`.
- Codex MCP support may vary by version — check the [Codex CLI releases](https://github.com/openai/codex/releases) for the minimum supported version.
