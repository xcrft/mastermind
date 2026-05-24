# Mastermind

A curated standard library of **skills**, **prompts**, **agent configs**, and **MCP integrations** for AI coding agents (primarily [Claude Code](https://claude.com/claude-code), portable to other tools).

The goal is to give teams and individuals a place to find — and contribute — battle-tested AI artifacts with a consistent shape, so adopting a new skill or prompt feels the same as the last one.

## What's inside

| Folder | What it holds |
|---|---|
| [`skills/`](skills/) | Markdown skills with frontmatter. Drop into `~/.claude/skills/` or a plugin. |
| [`prompts/`](prompts/) | Reusable system/user prompt templates for specific tasks. |
| [`agents/`](agents/) | Subagent definitions, CLAUDE.md templates, hooks, settings snippets. |
| [`mcp/`](mcp/) | MCP server configs and integration recipes. |

Each top-level folder is grouped by **domain** inside (e.g. `skills/code-review/`, `agents/subagents/`). Every category ships a `_template/` you copy when adding something new.

## Install

Pick the path that matches what you actually need.

### Path 1 — Claude Code plugin marketplace (recommended for end users)

One-time marketplace add, then install plugins by name. Skills + subagents land in `~/.claude/plugins/` automatically; the workflow CLAUDE.md template is bundled with the workflow plugin and you copy it into your project root yourself.

```bash
# In Claude Code:
/plugin marketplace add xcrft/mastermind
/plugin install mastermind-workflow@mastermind   # planner + critic + executor + auditor + researcher + release
/plugin install mmcg@mastermind                  # codegraph MCP server (needs the binary — see Path 2)
/plugin install mastermind-tools@mastermind      # standalone skills — pr-review, flaky-finder, doc-stub-sync
```

The `mmcg` plugin only registers the MCP server config (`.mcp.json`). The `mmcg` binary itself is a separate install — see Path 2.

The `plugins/` tree is regenerated from canonical artifacts by [`scripts/build-plugins.py`](scripts/build-plugins.py); the `.claude-plugin/` manifests are the marketplace entry points.

### Path 2 — `mmcg` binary (codegraph indexer)

`mmcg` is a standalone Rust binary that runs as an MCP server over stdio. Needed for any path that uses the truth layer.

```bash
# Once published on crates.io (see .github/workflows/publish-mmcg.yml):
cargo install mmcg

# Or, today (before first crates.io release), from git:
git clone https://github.com/xcrft/mastermind
cargo install --path mastermind/mcp/servers/mmcg
```

Installs `mmcg` into `~/.cargo/bin/`. Zero system dependencies — SQLite and tree-sitter grammars are bundled at build time. Needs Rust 1.75+.

To register `mmcg` with Claude Code manually (skip if you used the plugin in Path 1):

```jsonc
// ~/.claude/mcp.json
{
  "mcpServers": {
    "mmcg": {
      "command": "mmcg",
      "args": ["serve"],
      "env": { "MMCG_INDEX_PATH": ".mastermind/mmcg.db" }
    }
  }
}
```

### Path 3 — manual / hand mode (curate a subset)

Clone the repo and copy whatever artifacts you want into `~/.claude/` (per-user) or your project root (per-project). Use this when you want only a few subagents (e.g. just `mastermind-critic`) without the full workflow.

```bash
git clone https://github.com/xcrft/mastermind ~/mastermind
# Per-user skills (apply to every project)
cp mastermind/agents/subagents/mastermind-critic.md ~/.claude/agents/
# Or per-project (only this repo)
cp mastermind/agents/subagents/mastermind-critic.md /path/to/your/project/.claude/agents/
```

Read each artifact's frontmatter (`name`, `description`, `metadata.requires`) to know what it expects.

### Path 4 — bootstrap a new project (after `mmcg` is installed)

Inside any project's working directory:

```bash
mmcg init                       # scaffolds .mastermind/{tasks/, .gitignore, mmcg.db}, CONTEXT.md
mmcg init --with-claude-md      # also drops in the workflow CLAUDE.md template
mmcg index .                    # populates the code index
mmcg watch &                    # (optional) keeps the index fresh as you edit
```

Then add `.mastermind/` to your project's root `.gitignore` — everything under it is local working state (specs, mmcg index) and shouldn't be committed.

If you already have a pre-0.6.0 `.tasks/` directory at project root, `mmcg init` will print a migration command (`mv .tasks/* .mastermind/tasks/ && rmdir .tasks`).

## Contributing

The repo lives or dies by consistency. Before adding anything, read:

- [`docs/conventions.md`](docs/conventions.md) — naming, frontmatter, file layout rules. **This is the standard.**
- The `anatomy.md` for your artifact type (e.g. [`docs/skill-anatomy.md`](docs/skill-anatomy.md)).
- [`CONTRIBUTING.md`](CONTRIBUTING.md) — PR process.

To add a new artifact: copy the matching `_template/`, fill it in, open a PR.

## License

MIT — see [`LICENSE`](LICENSE).
