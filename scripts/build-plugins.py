#!/usr/bin/env python3
"""
Build the `plugins/` tree for Claude Code marketplace publishing.

Claude Code expects plugins to have flat `agents/`, `skills/`, `commands/`, `mcp/`
subdirectories — but our canonical layout uses domain-categorized folders
(`skills/code-review/pr-review/`, etc.). This script copies the canonical artifacts
into the flat plugin layout that Claude Code needs.

  Canonical                                  →  Plugin layout
  -------------------------------------------     ------------------------------
  skills/workflow/mastermind-task-planning/   →   plugins/mastermind-workflow/skills/mastermind-task-planning/
  agents/subagents/mastermind-critic.md       →   plugins/mastermind-workflow/agents/mastermind-critic.md
  agents/claude-md/mastermind-workflow.md     →   plugins/mastermind-workflow/CLAUDE.md
  mcp/servers/mmcg/                           →   plugins/mmcg/mcp/mmcg/

Run before `git commit` if you're publishing to the marketplace.

This script is idempotent: it removes the per-plugin `agents/`, `skills/`, `mcp/`
subdirectories first (preserving `.claude-plugin/plugin.json`), then re-copies.
"""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
PLUGINS_DIR = REPO_ROOT / "plugins"


def reset_plugin_content(plugin_name: str) -> Path:
    """Wipe the per-plugin content dirs, preserving the plugin manifest."""
    plugin_dir = PLUGINS_DIR / plugin_name
    plugin_dir.mkdir(parents=True, exist_ok=True)
    for sub in ("agents", "skills", "commands", "mcp", "docs"):
        target = plugin_dir / sub
        if target.exists():
            shutil.rmtree(target)
    return plugin_dir


def copy_tree(src: Path, dst: Path) -> None:
    """Copy a directory tree, creating parents as needed."""
    dst.parent.mkdir(parents=True, exist_ok=True)
    if src.is_dir():
        shutil.copytree(src, dst)
    else:
        shutil.copy2(src, dst)


def build_mastermind_workflow() -> None:
    """Workflow plugin: planner+executor skills + 5 subagents + 1 CLAUDE.md template."""
    plugin = reset_plugin_content("mastermind-workflow")
    (plugin / "skills").mkdir()
    (plugin / "agents").mkdir()

    # Skills
    copy_tree(
        REPO_ROOT / "skills/workflow/mastermind-task-planning",
        plugin / "skills/mastermind-task-planning",
    )
    copy_tree(
        REPO_ROOT / "skills/workflow/mastermind-task-executor",
        plugin / "skills/mastermind-task-executor",
    )
    copy_tree(
        REPO_ROOT / "skills/prompt-engineering/mastermind-prompt-refiner",
        plugin / "skills/mastermind-prompt-refiner",
    )

    # Subagents
    for name in (
        "mastermind-critic.md",
        "mastermind-auditor.md",
        "mastermind-researcher.md",
        "mastermind-task-executor.md",
        "mastermind-prompt-refiner.md",
        "mastermind-release.md",
    ):
        copy_tree(REPO_ROOT / "agents/subagents" / name, plugin / "agents" / name)

    # CLAUDE.md template lands at the plugin root so adopters get a hint
    copy_tree(
        REPO_ROOT / "agents/claude-md/mastermind-workflow.md",
        plugin / "CLAUDE.md",
    )
    copy_tree(
        REPO_ROOT / "agents/claude-md/mastermind-context.md",
        plugin / "CONTEXT.md.template",
    )


def build_mmcg() -> None:
    """mmcg plugin: just the MCP server config — adopter installs the binary via cargo.

    Claude Code plugins integrate MCP servers via a single `.mcp.json` at the plugin
    root (not a `mcp/` subdirectory). The plugin's job is to register the server
    config; the binary itself lives wherever the adopter installs it (`cargo install`).
    """
    plugin = reset_plugin_content("mmcg")
    mcp_config = {
        "mmcg": {
            "command": "mmcg",
            "args": ["serve"],
            "env": {
                "MMCG_INDEX_PATH": ".mastermind/mmcg.db"
            }
        }
    }
    import json
    (plugin / ".mcp.json").write_text(
        json.dumps(mcp_config, indent=2) + "\n", encoding="utf-8"
    )
    # Include a README explaining the install
    install_doc = (
        "# mmcg plugin\n\n"
        "This plugin registers the **mmcg** MCP server. You need to install the\n"
        "`mmcg` binary separately — the plugin only provides the server config.\n\n"
        "## Install the binary\n\n"
        "```bash\n"
        "git clone https://github.com/xcrft/mastermind\n"
        "cd mastermind/mcp/servers/mmcg\n"
        "cargo install --path .\n"
        "```\n\n"
        "This puts `mmcg` in `~/.cargo/bin/`. The plugin's `.mcp.json` runs `mmcg serve`\n"
        "expecting that binary on PATH.\n\n"
        "## Usage\n\n"
        "After installing this plugin and the binary:\n\n"
        "```bash\n"
        "cd your-project\n"
        "mmcg init        # scaffold .mastermind/tasks/ + .mastermind/ + CONTEXT.md\n"
        "mmcg index .     # populate the index\n"
        "mmcg watch &     # keep the index fresh (optional)\n"
        "```\n\n"
        "The MCP server reads/writes `.mastermind/mmcg.db` in the project root.\n"
    )
    (plugin / "README.md").write_text(install_doc, encoding="utf-8")


def build_mastermind_tools() -> None:
    """Standalone tools plugin: pr-review, flaky-finder, doc-stub-sync skills + 2 prompts."""
    plugin = reset_plugin_content("mastermind-tools")
    (plugin / "skills").mkdir()

    copy_tree(
        REPO_ROOT / "skills/code-review/pr-review",
        plugin / "skills/pr-review",
    )
    copy_tree(
        REPO_ROOT / "skills/testing/flaky-finder",
        plugin / "skills/flaky-finder",
    )
    copy_tree(
        REPO_ROOT / "skills/docs/doc-stub-sync",
        plugin / "skills/doc-stub-sync",
    )
    # Prompts aren't a Claude Code first-class type — drop them as docs/
    docs_dir = plugin / "docs"
    docs_dir.mkdir()
    copy_tree(
        REPO_ROOT / "prompts/workflow/senior-eng-review.md",
        docs_dir / "senior-eng-review.md",
    )
    copy_tree(
        REPO_ROOT / "prompts/workflow/api-shape-explorer.md",
        docs_dir / "api-shape-explorer.md",
    )


def main() -> int:
    if not PLUGINS_DIR.exists():
        PLUGINS_DIR.mkdir()

    print(f"Building plugins/ from canonical artifacts in {REPO_ROOT.name}/...")
    build_mastermind_workflow()
    print("  ✓ plugins/mastermind-workflow/")
    build_mmcg()
    print("  ✓ plugins/mmcg/")
    build_mastermind_tools()
    print("  ✓ plugins/mastermind-tools/")

    print(f"\nDone. {len(list(PLUGINS_DIR.iterdir()))} plugins ready.")
    print("Next steps:")
    print("  1. Review the generated content with `git status plugins/`")
    print("  2. Commit if it looks right")
    print("  3. The marketplace is discoverable at this repo's root via .claude-plugin/marketplace.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
