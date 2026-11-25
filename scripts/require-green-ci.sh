#!/usr/bin/env bash
# Wait until the push CI of one commit has finished, and fail unless every
# required workflow succeeded. Used by the publish workflows and by
# `just release-tags`, so a release never depends on someone checking CI by hand.
#
#   GH_TOKEN=... scripts/require-green-ci.sh <repo> <commit-sha>
#
# CI_WAIT_SECONDS (default 3600) bounds the wait, CI_POLL_SECONDS (default 60)
# sets the interval.
set -euo pipefail

repo=${1:?usage: require-green-ci.sh <owner/repo> <commit-sha>}
sha=${2:?usage: require-green-ci.sh <owner/repo> <commit-sha>}
wait_seconds=${CI_WAIT_SECONDS:-3600}
poll_seconds=${CI_POLL_SECONDS:-60}
workflows=(ci-mmcg.yml ci-npm.yml validate.yml supply-chain.yml)
deadline=$((SECONDS + wait_seconds))

while true; do
  pending=()
  for workflow in "${workflows[@]}"; do
    result=$(gh run list --repo "$repo" --commit "$sha" --workflow "$workflow" --event push \
      --limit 1 --json status,conclusion --jq '.[0] | "\(.status) \(.conclusion)"')
    case "$result" in
      "completed success") ;;
      completed\ *)
        echo "::error::$workflow finished as '${result#completed }' on $sha" >&2
        exit 1
        ;;
      *) pending+=("$workflow") ;;
    esac
  done
  if [ ${#pending[@]} -eq 0 ]; then
    echo "CI is green on $sha"
    exit 0
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "::error::CI did not finish on $sha within ${wait_seconds}s: ${pending[*]}" >&2
    exit 1
  fi
  echo "waiting for ${pending[*]} on $sha"
  sleep "$poll_seconds"
done
