#!/usr/bin/env sh
# Mastermind one-shot installer.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/xcrft/mastermind/main/install.sh | sh
#
# Or, from a local checkout:
#   ./install.sh
#
# What it does:
#   1. Builds and installs the `mmcg` binary into ~/.cargo/bin (needs Rust ≥ 1.75)
#   2. Copies all subagents, skills, prompts, and CLAUDE.md templates into ~/.claude/
#   3. Points you at `mmcg setup claude --write-mcp` for the MCP registration step
#      (single source of truth — see `mmcg setup claude --help`).
#
# NOTE: Earlier versions of this script wrote `~/.claude/mcp.json` directly. That
# path drifted from what `mmcg doctor` and `mmcg setup claude` look for
# (`~/.claude/.mcp.json` with a dot prefix). The MCP write step is now delegated
# to `mmcg setup claude` so there is exactly one code path for config writing.
#
# Override CLAUDE_HOME to install somewhere else: CLAUDE_HOME=/tmp/test ./install.sh
# Override REPO_URL if you fork: REPO_URL=https://github.com/you/mastermind ./install.sh

set -e

REPO_URL="${REPO_URL:-https://github.com/xcrft/mastermind}"
CLAUDE_DIR="${CLAUDE_HOME:-$HOME/.claude}"
SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" 2>/dev/null && pwd)"

# Detect: are we running from the repo (./install.sh), or curl-piped (no script dir)?
if [ -n "$SCRIPT_DIR" ] && [ -f "$SCRIPT_DIR/mcp/servers/mmcg/Cargo.toml" ]; then
    SRC="$SCRIPT_DIR"
    CLEANUP_SRC=0
else
    TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t mastermind)"
    SRC="$TMP_DIR/mastermind"
    CLEANUP_SRC=1
    printf '→ Cloning %s ...\n' "$REPO_URL"
    git clone --depth=1 --quiet "$REPO_URL" "$SRC"
fi

# 1. mmcg binary
printf '\n→ Installing mmcg binary ...\n'
if ! command -v cargo >/dev/null 2>&1; then
    printf '  ✗ cargo not found. Install Rust first: https://rustup.rs/\n'
    printf '    Then re-run this installer.\n'
    [ "$CLEANUP_SRC" = 1 ] && /bin/rm -rf "$TMP_DIR"
    exit 1
fi
cargo install --path "$SRC/mcp/servers/mmcg" --quiet --locked
MMCG_BIN="$(command -v mmcg)"
printf '  ✓ mmcg → %s\n' "$MMCG_BIN"

# 2. Artifacts → CLAUDE_DIR
printf '\n→ Installing artifacts → %s\n' "$CLAUDE_DIR"
mkdir -p "$CLAUDE_DIR/agents" "$CLAUDE_DIR/skills" "$CLAUDE_DIR/prompts" "$CLAUDE_DIR/templates"

# 2a. Subagents — flat copy from agents/subagents/*.md
n_agents=0
for f in "$SRC"/agents/subagents/*.md; do
    [ -f "$f" ] || continue
    cp -f "$f" "$CLAUDE_DIR/agents/"
    n_agents=$((n_agents + 1))
done
printf '  ✓ %d subagents → %s/agents/\n' "$n_agents" "$CLAUDE_DIR"

# 2b. Skills — preserve domain/<skill-name>/ structure but skip _template
n_skills=0
for skill_dir in "$SRC"/skills/*/*/; do
    name="$(basename "$skill_dir")"
    [ "$name" = "_template" ] && continue
    [ -f "$skill_dir/SKILL.md" ] || continue
    # Remove old version to avoid stale references files
    /bin/rm -rf "$CLAUDE_DIR/skills/$name"
    cp -R "$skill_dir" "$CLAUDE_DIR/skills/$name/"
    n_skills=$((n_skills + 1))
done
printf '  ✓ %d skills → %s/skills/\n' "$n_skills" "$CLAUDE_DIR"

# 2c. Prompts — flat copy from prompts/workflow/
n_prompts=0
for f in "$SRC"/prompts/workflow/*.md; do
    [ -f "$f" ] || continue
    cp -f "$f" "$CLAUDE_DIR/prompts/"
    n_prompts=$((n_prompts + 1))
done
printf '  ✓ %d prompts → %s/prompts/\n' "$n_prompts" "$CLAUDE_DIR"

# 2d. CLAUDE.md templates — copy as templates (do NOT overwrite ~/.claude/CLAUDE.md)
n_templates=0
for f in "$SRC"/agents/claude-md/*.md; do
    [ -f "$f" ] || continue
    cp -f "$f" "$CLAUDE_DIR/templates/"
    n_templates=$((n_templates + 1))
done
printf '  ✓ %d CLAUDE.md templates → %s/templates/\n' "$n_templates" "$CLAUDE_DIR"

# 3. MCP registration — delegated to `mmcg setup claude` so there's exactly
#    one code path for config writing (this script used to write its own,
#    drifted from what `mmcg doctor` and `mmcg setup claude` look for, and
#    quietly broke).
printf '\n→ MCP server config\n'
printf '  ℹ This installer no longer writes MCP config directly.\n'
printf '    Run one of:\n'
printf '      mmcg setup claude --write-mcp                        # user scope → ~/.claude.json\n'
printf '      mmcg setup claude --project . --write-mcp            # project-local ./.mcp.json\n'
printf '      mmcg setup claude                                    # dry-run: print diff and exit\n\n'
printf '    See `mmcg setup claude --help` for `--with-workflow` (drop CLAUDE.md template)\n'
printf '    and `--force` (overwrite a customized entry).\n'

# Cleanup temp clone if we made one
if [ "$CLEANUP_SRC" = 1 ]; then
    /bin/rm -rf "$TMP_DIR"
fi

printf '\n✓ Mastermind installed.\n\n'
printf 'Next steps for a project:\n'
printf '  cd your-project\n'
printf '  mmcg init                     # scaffold .mastermind/{tasks,mmcg.db}\n'
printf '  mmcg index .                  # populate the codegraph\n'
printf '  echo .mastermind/ >> .gitignore\n\n'
printf 'To adopt the planner/critic/executor/auditor workflow in a project:\n'
printf '  cp %s/templates/mastermind-workflow.md /path/to/project/CLAUDE.md\n' "$CLAUDE_DIR"
printf '  cp %s/templates/mastermind-context.md  /path/to/project/CONTEXT.md\n\n' "$CLAUDE_DIR"
printf 'Re-run this installer anytime to refresh artifacts.\n'
