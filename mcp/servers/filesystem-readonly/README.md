---
name: filesystem-readonly
description: Read-only filesystem MCP server — exposes file read and directory listing tools without write access. Use when an agent needs to inspect files outside its working directory and you want zero risk of modification.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - filesystem
    - readonly
  transport: stdio
  source: https://github.com/modelcontextprotocol/servers/tree/main/src/filesystem
---

# Filesystem (read-only)

A thin wrapper over the official `@modelcontextprotocol/server-filesystem` that disables every write tool. Use it to let an agent browse a directory tree for context — logs, sibling repos, reference docs — without write access.

## Tools exposed

- `read_file(path)` — return file contents as text
- `read_multiple_files(paths)` — batch read, one error per missing file
- `list_directory(path)` — list immediate children with file/dir type
- `directory_tree(path)` — recursive listing as a JSON tree
- `search_files(path, pattern)` — glob-style search inside `path`
- `get_file_info(path)` — size, mtime, type

**Disabled (no write tools):** `write_file`, `edit_file`, `create_directory`, `move_file`, `delete_file`.

## Install

```bash
# No install step — npx fetches it on demand
# Just configure your MCP client (see below)
```

## Configure

Add to your MCP config (e.g., `~/.claude/mcp.json` or `claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "filesystem-readonly": {
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "${READ_ROOT}"
      ],
      "env": {}
    }
  }
}
```

> **Why this is read-only despite using the standard server:** the standard server respects the directory list in `args` as its allowlist. We pass exactly one root and trust the client to never invoke the write tools. If you need a *hard* read-only guarantee (e.g., compromised model), run the directory mount with OS-level read-only permissions or use a sandboxed wrapper.

## Env vars

| Var | Required | Default | What it does |
|---|---|---|---|
| `READ_ROOT` | yes | — | Absolute path the server is allowed to read from. Anything outside this tree returns "access denied". |

## Notes

- Multiple roots: pass them as separate args. `"args": ["-y", "@modelcontextprotocol/server-filesystem", "/path/a", "/path/b"]`.
- For true write-prevention against a misbehaving agent: mount the directory read-only at the filesystem level (`mount -o ro` on Linux, APFS read-only snapshot on macOS), don't rely on the MCP allowlist.
- Companion: pair with `skills/code-review/pr-review` when reviewing PRs that reference files outside the working directory.
