# Cursor integration

Cursor supports MCP servers via `.cursor/mcp.json` (project-local) or `~/.cursor/mcp.json` (global).

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

**3. Register the MCP server**

Project-local (recommended — version-pinned per project):

```bash
mkdir -p .cursor
cat > .cursor/mcp.json << 'EOF'
{
  "mcpServers": {
    "mmcg": {
      "command": "mastermind",
      "args": ["serve"]
    }
  }
}
EOF
```

Global (available in all Cursor projects):

```bash
mkdir -p ~/.cursor
cat > ~/.cursor/mcp.json << 'EOF'
{
  "mcpServers": {
    "mmcg": {
      "command": "mastermind",
      "args": ["serve"]
    }
  }
}
EOF
```

**4. Restart Cursor**

MCP servers are loaded at startup. After restarting, the 20 codegraph tools (`mmcg_search`, `mmcg_callers`, `mmcg_impact`, etc.) are available to Cursor's AI.

## Using a project-local binary

If you install Mastermind as a dev dependency to pin the version:

```bash
npm install -D @xcraftmind/mastermind
```

Update the MCP config to use the local binary:

```json
{
  "mcpServers": {
    "mmcg": {
      "command": "./node_modules/.bin/mastermind",
      "args": ["serve"]
    }
  }
}
```

## Keeping the index current

The codegraph index lives at `.mastermind/mmcg.db`. Re-index after significant changes:

```bash
mastermind index .
```

Or run the watcher to re-index automatically on file save:

```bash
mastermind watch
```

## Notes

- The workflow subagents (planner, auditor, critic, etc.) are Claude Code-specific and not applicable to Cursor.
- mmcg tools work with any MCP-capable AI in Cursor — Cursor's built-in models, GPT-4, Claude, etc.
- Add `.mastermind/` to `.gitignore` — the index is local state and should not be committed.
