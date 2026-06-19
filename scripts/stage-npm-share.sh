#!/usr/bin/env bash
# Stage the runtime workflow artifacts (subagents + skills) into the npm root
# package so `mastermind init` can install them into ~/.claude/. The staged
# `npm/mastermind/share/` tree is gitignored and (re)built from the canonical
# `agents/subagents/` + `skills/` trees by this script. Run it:
#   - in the publish workflow (assemble) and the ci-npm smoke, before `npm pack`
#   - locally, before testing `init`'s global install
#
# Skills are allowlisted: only the core skills below ship in the default install.
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
  skills/workflow/mastermind-codegraph-research
  skills/workflow/mastermind-structured-report-contract
  skills/workflow/mastermind-critical-review
  skills/prompt-engineering/mastermind-prompt-refiner
  skills/debugging/mastermind-investigation-ledger
  skills/security/mastermind-agent-security-review
  skills/coding/no-ai-slop-comments
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
