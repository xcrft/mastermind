#!/usr/bin/env bash
# Stage the runtime workflow artifacts (subagents + skills) into the npm root
# package so `mastermind install` and `mastermind init` can install them into
# the Claude and Codex workflow homes. The staged
# `npm/mastermind/share/` tree is gitignored and (re)built from the canonical
# `agents/subagents/` + `skills/` trees by this script. Run it:
#   - in the publish workflow (assemble) and the ci-npm smoke, before `npm pack`
#   - locally, before testing `init`'s global install
#
# Every skill under `skills/` ships in the default install.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SHARE="$REPO_ROOT/npm/mastermind/share"

rm -rf "$SHARE"
mkdir -p "$SHARE/agents" "$SHARE/skills"

# Subagents — flat `.md` files.
cp "$REPO_ROOT"/agents/subagents/*.md "$SHARE/agents/"

# Every skill under `skills/` ships automatically — no allowlist to forget.
while IFS= read -r skill_md; do
  src="$(dirname "$skill_md")"
  cp -R "$src" "$SHARE/skills/$(basename "$src")"
done < <(find "$REPO_ROOT/skills" -name SKILL.md | sort)

agents_n=$(find "$SHARE/agents" -name '*.md' | wc -l | tr -d ' ')
skills_n=$(find "$SHARE/skills" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
if [ "$agents_n" -eq 0 ] || [ "$skills_n" -eq 0 ]; then
  echo "ERROR: staged $agents_n subagents + $skills_n skills — discovery found nothing" >&2
  exit 1
fi
echo "staged $agents_n subagents + $skills_n skills → npm/mastermind/share/"
