---
name: mastermind-cross-client-setup
description: Configure, inspect, update, or remove Mastermind MCP registration for Claude Code, Cursor, Codex, Continue, or an explicit generic config. Use when a user asks to install Mastermind in an MCP client, choose project/user scope, preview a safe config change, troubleshoot setup, or verify registration with doctor.
metadata:
  version: 0.1.0
  authors: [mastermind]
  tags: [workflow, mcp, setup]
---

# Mastermind Cross-Client Setup

Use the built-in adapter. Do not hand-edit client configuration unless the
adapter reports an unsupported case.

## Workflow

1. Resolve client and scope from the request. Respect the capability matrix:
   Codex is user-scope only; Generic requires an explicit config path; Continue
   owns its standalone Mastermind YAML file.
2. Run a dry-run first:

   ```bash
   mastermind setup CLIENT --scope SCOPE --root . [--config PATH]
   ```

3. Explain only the redacted action summary. Never print existing config,
   environment values, unknown fields, or subprocess output.
4. If the user authorized installation/update/removal and the preview matches,
   repeat with `--write`. Add `--remove` only for removal.
5. Use `--force` only after explicitly showing that a customized Mastermind
   entry will be backed up and replaced/removed. Never use force to imply write.
6. Run `mastermind doctor ROOT`, using the same root passed to setup, and report
   structural config status separately from the trusted current-binary MCP
   handshake. On a new project without `.mastermind/mmcg.db`, explain that the
   config can be canonical while the overall doctor exits nonzero and skips the
   handshake; index the project before requiring an all-green doctor.

Preserve unrelated servers/settings. Stop on malformed/duplicate configs,
symlinks, concurrent edits, unsupported scope, missing native CLI, or unsafe
path resolution. Do not execute the command stored in client configuration.
