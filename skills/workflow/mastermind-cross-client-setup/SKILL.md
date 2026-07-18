---
name: mastermind-cross-client-setup
description: Install portable Mastermind workflow adapters and configure, inspect, update, or remove MCP registration for Claude Code, Cursor, Codex, Continue, or an explicit generic config. Use when a user asks to install Mastermind in an AI coding client, choose project/user scope, preview a safe config change, troubleshoot setup, or verify workflow/MCP parity with doctor.
metadata:
  version: 0.2.0
  authors: [mastermind]
  tags: [workflow, mcp, setup]
---

# Mastermind Cross-Client Setup

Use the built-in adapter. Do not hand-edit client configuration unless the
adapter reports an unsupported case.

## Workflow

1. Separate the workflow bundle from MCP registration. For Claude or Codex
   workflow skills, use `mastermind install --client CLIENT`; use `--client
   all` for both. Claude also receives spawnable subagents. Verify installed
   ownership and SHA-256 content parity with
   `mastermind doctor --workflow --client CLIENT`.
2. Resolve MCP client and scope from the request. Respect the capability matrix:
   Codex is user-scope only; Generic requires an explicit config path; Continue
   owns its standalone Mastermind YAML file.
3. Run an MCP dry-run first:

   ```bash
   mastermind setup CLIENT --scope SCOPE --root . [--config PATH]
   ```

4. Explain only the redacted action summary. Never print existing config,
   environment values, unknown fields, or subprocess output.
5. If the user authorized installation/update/removal and the preview matches,
   repeat with `--write`. Add `--remove` only for removal.
6. Use `--force` only after explicitly showing that a customized Mastermind
   entry will be backed up and replaced/removed. Never use force to imply write.
7. Run `mastermind doctor ROOT`, using the same root passed to setup, and report
   structural config status separately from the trusted current-binary MCP
   handshake. On a new project without `.mastermind/mmcg.db`, explain that the
   config can be canonical while the overall doctor exits nonzero and skips the
   handshake; index the project before requiring an all-green doctor.

Preserve unrelated servers/settings. Stop on malformed/duplicate configs,
symlinks, concurrent edits, unsupported scope, missing native CLI, or unsafe
path resolution. Do not execute the command stored in client configuration.
