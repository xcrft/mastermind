# Continue integration

[Continue](https://continue.dev) supports MCP servers via `~/.continue/config.json`.

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

**3. Add mmcg to Continue config**

Edit `~/.continue/config.json` and add an `experimental.modelContextProtocolServers` entry:

```json
{
  "experimental": {
    "modelContextProtocolServers": [
      {
        "transport": {
          "type": "stdio",
          "command": "mastermind",
          "args": ["serve"]
        }
      }
    ]
  }
}
```

If the `experimental` key doesn't exist, add it at the top level alongside your `models` config.

**4. Reload Continue**

Use the Continue reload command (`Cmd/Ctrl+Shift+P` → "Continue: Reload") or restart your editor. The codegraph tools should appear in Continue's tool list.

## Scoping to a project index

By default `mastermind serve` uses `.mastermind/mmcg.db` relative to the current working directory — whichever directory your editor launched from. To explicitly point to a project's index:

```json
{
  "experimental": {
    "modelContextProtocolServers": [
      {
        "transport": {
          "type": "stdio",
          "command": "mastermind",
          "args": ["--index", "/path/to/your-project/.mastermind/mmcg.db", "serve"]
        }
      }
    ]
  }
}
```

## Notes

- The workflow subagents (planner, auditor, critic, etc.) are Claude Code-specific.
- mmcg tools are available to any model configured in Continue.
- The Continue MCP integration is experimental — check the [Continue changelog](https://github.com/continuedev/continue/releases) for API changes.
- Add `.mastermind/` to `.gitignore`.
