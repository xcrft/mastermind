#!/usr/bin/env bash
# Stage the runtime workflow artifacts (subagents + skills) into the npm root
# package so `mastermind init` can install them into ~/.claude/. The staged
# `npm/mastermind/share/` tree is gitignored and (re)built from the canonical
# `agents/subagents/` + `skills/` trees by this script. Run it:
#   - in the publish workflow (assemble) and the ci-npm smoke, before `npm pack`
#   - locally, before testing `init`'s global install
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHARE="$REPO_ROOT/npm/mastermind/share"

rm -rf "$SHARE"
mkdir -p "$SHARE/agents" "$SHARE/skills"

# Subagents — flat `.md` files.
cp "$REPO_ROOT"/agents/subagents/*.md "$SHARE/agents/"

# Skills — directories with a SKILL.md (+ optional references/). Skip _template.
while IFS= read -r skill_md; do
    dir="$(dirname "$skill_md")"
    name="$(basename "$dir")"
    [ "$name" = "_template" ] && continue
    cp -R "$dir" "$SHARE/skills/$name"
done < <(find "$REPO_ROOT/skills" -name SKILL.md)

agents_n=$(find "$SHARE/agents" -name '*.md' | wc -l | tr -d ' ')
skills_n=$(find "$SHARE/skills" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
echo "staged $agents_n subagents + $skills_n skills → npm/mastermind/share/"
