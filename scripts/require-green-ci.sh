#!/usr/bin/env bash
# Wait until the runs of one commit have finished, and fail unless every listed
# workflow succeeded. By default it checks the required push CI. The publish
# workflows and `just release-tags` use it, so a release never depends on
# someone checking CI by hand.
#
#   GH_TOKEN=... scripts/require-green-ci.sh <repo> <commit-sha>
#   GH_TOKEN=... scripts/require-green-ci.sh <repo> <commit-sha> <event> <workflow>...
#
# The second form checks the newest run of each named workflow for that event,
# for example the `workflow_dispatch` dry runs of the publish workflows.
#
# CI_WAIT_SECONDS (default 3600) bounds the wait, CI_POLL_SECONDS (default 60)
# sets the interval.
set -euo pipefail

usage="usage: require-green-ci.sh <owner/repo> <commit-sha> [<event> <workflow>...]"
if [ "$#" -ne 2 ] && [ "$#" -lt 4 ]; then
  echo "$usage" >&2
  exit 2
fi
repo=$1
sha=$2
if [ "$#" -ge 4 ]; then
  event=$3
  workflows=("${@:4}")
else
  event=push
  workflows=(ci-mmcg.yml ci-npm.yml validate.yml supply-chain.yml)
fi
wait_seconds=${CI_WAIT_SECONDS:-3600}
poll_seconds=${CI_POLL_SECONDS:-60}
deadline=$((SECONDS + wait_seconds))

while true; do
  pending=()
  for workflow in "${workflows[@]}"; do
    result=$(gh run list --repo "$repo" --commit "$sha" --workflow "$workflow" --event "$event" \
      --limit 1 --json status,conclusion --jq '.[0] | "\(.status) \(.conclusion)"')
    case "$result" in
      "completed success") ;;
      completed\ *)
        echo "::error::$workflow finished as '${result#completed }' on $sha ($event)" >&2
        exit 1
        ;;
      *) pending+=("$workflow") ;;
    esac
  done
  if [ ${#pending[@]} -eq 0 ]; then
    echo "$event runs are green on $sha: ${workflows[*]}"
    exit 0
  fi
  if [ "$SECONDS" -ge "$deadline" ]; then
    echo "::error::$event runs did not finish on $sha within ${wait_seconds}s: ${pending[*]}" >&2
    exit 1
  fi
  echo "waiting for ${pending[*]} ($event) on $sha"
  sleep "$poll_seconds"
done
