#!/usr/bin/env bash
# Stage the runtime workflow artifacts (subagents + skills) into the npm root
# package so `mastermind init` can install them into ~/.claude/. The staged
# `npm/mastermind/share/` tree is gitignored and (re)built from the canonical
# `agents/subagents/` + `skills/` trees by this script. Run it:
#   - in the publish workflow (assemble) and the ci-npm smoke, before `npm pack`
#   - locally, before testing `init`'s global install
#
# Skills are allowlisted: only the three core skills ship in the default install.
# Non-core skills live in extras/ and are not copied here.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHARE="$REPO_ROOT/npm/mastermind/share"

rm -rf "$SHARE"
mkdir -p "$SHARE/agents" "$SHARE/skills"

# Subagents — flat `.md` files.
cp "$REPO_ROOT"/agents/subagents/*.md "$SHARE/agents/"

# Core skills — explicit allowlist. Add here only skills that belong in the
# default install (intake → plan → execute → audit loop).
CORE_SKILLS=(
  skills/workflow/mastermind-task-planning
  skills/workflow/mastermind-task-executor
  skills/prompt-engineering/mastermind-prompt-refiner
)

for skill_dir in "${CORE_SKILLS[@]}"; do
  src="$REPO_ROOT/$skill_dir"
  name="$(basename "$skill_dir")"
  if [ ! -d "$src" ]; then
    echo "ERROR: core skill not found: $src" >&2
    exit 1
  fi
  cp -R "$src" "$SHARE/skills/$name"
done

agents_n=$(find "$SHARE/agents" -name '*.md' | wc -l | tr -d ' ')
skills_n=$(find "$SHARE/skills" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
echo "staged $agents_n subagents + $skills_n skills → npm/mastermind/share/"
