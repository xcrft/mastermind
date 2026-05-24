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
#   3. Prints an mmcg MCP-server snippet for you to paste into ~/.claude/mcp.json
#      (does NOT touch your mcp.json — too risky to merge JSON blindly)
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

# 3. MCP server snippet
printf '\n→ MCP server config\n'
MCP_FILE="$CLAUDE_DIR/mcp.json"
SNIPPET='{
  "mcpServers": {
    "mmcg": {
      "command": "mmcg",
      "args": ["serve"],
      "env": { "MMCG_INDEX_PATH": ".mastermind/mmcg.db" }
    }
  }
}'

if [ -f "$MCP_FILE" ]; then
    printf '  ⚠ %s already exists — not touching it.\n' "$MCP_FILE"
    if grep -q '"mmcg"' "$MCP_FILE" 2>/dev/null; then
        printf '    Looks like mmcg is already registered. Done.\n'
    else
        printf '    Add this block to its "mcpServers":\n\n'
        printf '%s\n\n' "$SNIPPET"
    fi
else
    printf '%s\n' "$SNIPPET" > "$MCP_FILE"
    printf '  ✓ wrote %s\n' "$MCP_FILE"
fi

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
