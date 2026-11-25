#!/usr/bin/env bash
# Apply the repository settings that release workflows cannot enforce from Git.
# Dry-run by default. Requires an admin-authenticated `gh` session for --apply.

set -euo pipefail

repository="xcrft/mastermind"
reviewer="aglumova"
apply=false
prevent_self_review=false
api_version="2026-03-10"

usage() {
    echo "usage: $0 [--repository owner/repo] [--reviewer login] [--prevent-self-review] [--apply]" >&2
}

while [ "$#" -gt 0 ]; do
    case "$1" in
        --repository)
            [ "$#" -ge 2 ] || { usage; exit 2; }
            repository="$2"
            shift 2
            ;;
        --reviewer)
            [ "$#" -ge 2 ] || { usage; exit 2; }
            reviewer="$2"
            shift 2
            ;;
        --apply)
            apply=true
            shift
            ;;
        --prevent-self-review)
            prevent_self_review=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            usage
            exit 2
            ;;
    esac
done

[[ "$repository" =~ ^[A-Za-z0-9._-]+/[A-Za-z0-9._-]+$ ]] || {
    echo "error: repository must be owner/repo" >&2
    exit 2
}
[[ "$reviewer" =~ ^[A-Za-z0-9-]+$ ]] || {
    echo "error: reviewer must be a GitHub login" >&2
    exit 2
}
command -v gh >/dev/null || { echo "error: gh is required" >&2; exit 2; }
command -v jq >/dev/null || { echo "error: jq is required" >&2; exit 2; }

permission=$(gh repo view "$repository" --json viewerPermission --jq .viewerPermission 2>/dev/null || echo UNKNOWN)
if ! "$apply"; then
    cat <<EOF
Dry run: no GitHub settings changed.
Repository: $repository
Authenticated permission: $permission
Would configure:
  - npm-prod and prod required_reviewers: $reviewer
  - npm-prod and prod prevent_self_review: $prevent_self_review
  - npm-prod deploys only from npm-v* tags, prod only from mmcg-v* tags
  - active release tag rulesets: refs/tags/npm-v* and refs/tags/mmcg-v*
  - required main checks: validator, Rust, npm smoke, cargo-deny
Re-run with --apply using a repository-admin gh session.
Enable --prevent-self-review only with an eligible reviewer different from the workflow initiator.
EOF
    exit 0
fi

if [ "$permission" != "ADMIN" ]; then
    echo "error: --apply requires ADMIN permission on $repository (current: $permission)" >&2
    exit 1
fi

if "$prevent_self_review"; then
    current_actor=$(gh api user --jq .login)
    if [ "$reviewer" = "$current_actor" ]; then
        echo "error: --prevent-self-review requires an eligible reviewer different from the workflow initiator (authenticated actor: $current_actor)" >&2
        exit 1
    fi
fi

api() {
    gh api -H "X-GitHub-Api-Version: $api_version" "$@"
}

reviewer_id=$(api "users/$reviewer" --jq .id)

# One release boundary per registry: an environment that only its own tags may
# deploy from, and a tag ruleset that makes those tags immutable.
# Environment secrets are never read or replaced.
protect_release() {
    local environment="$1" prefix="$2" ruleset_name="$3"
    jq -n \
      --argjson reviewer_id "$reviewer_id" \
      --argjson prevent_self_review "$prevent_self_review" '{
      wait_timer: 0,
      prevent_self_review: $prevent_self_review,
      reviewers: [{type: "User", id: $reviewer_id}],
      deployment_branch_policy: {
        protected_branches: false,
        custom_branch_policies: true
      }
    }' | api --method PUT "repos/$repository/environments/$environment" --input - >/dev/null

    local policies="repos/$repository/environments/$environment/deployment-branch-policies"
    # Any branch policy would let a non-tag ref deploy, so only the tag policy stays.
    api "$policies" --jq ".branch_policies[]? | select(.name != \"$prefix*\" or .type != \"tag\") | .id" \
        | while read -r policy_id; do
            api --method DELETE "$policies/$policy_id" >/dev/null
        done
    if ! api "$policies" --jq ".branch_policies[]? | select(.name == \"$prefix*\" and .type == \"tag\") | .id" | grep -q .; then
        api --method POST "$policies" -f name="$prefix*" -f type='tag' >/dev/null
    fi

    local ruleset_id
    ruleset_id=$(api "repos/$repository/rulesets" \
        --jq ".[] | select(.name == \"$ruleset_name\" and .target == \"tag\") | .id" | head -n 1)
    local payload
    payload=$(jq -n --arg name "$ruleset_name" --arg ref "refs/tags/$prefix*" '{
      name: $name,
      target: "tag",
      enforcement: "active",
      bypass_actors: [{actor_id: null, actor_type: "OrganizationAdmin", bypass_mode: "always"}],
      conditions: {ref_name: {include: [$ref], exclude: []}},
      rules: [
        {type: "deletion"},
        {type: "non_fast_forward"},
        {type: "update"},
        {type: "creation"}
      ]
    }')
    if [ -n "$ruleset_id" ]; then
        printf '%s' "$payload" | api --method PUT "repos/$repository/rulesets/$ruleset_id" --input - >/dev/null
    else
        printf '%s' "$payload" | api --method POST "repos/$repository/rulesets" --input - >/dev/null
    fi
}

protect_release npm-prod npm-v "npm release tags"
protect_release prod mmcg-v "mmcg release tags"

main_ruleset_id=$(api "repos/$repository/rulesets" \
    --jq '.[] | select(.name == "main" and .target == "branch") | .id' \
    | head -n 1)
if [ -z "$main_ruleset_id" ]; then
    echo "error: active main branch ruleset not found" >&2
    exit 1
fi
required_checks=$(jq -nc '[
  {context: "Frontmatter & cross-references"},
  {context: "cargo test + clippy + fmt"},
  {context: "wrapper check + linux-x64 install smoke"},
  {context: "advisories, licenses, bans, sources"}
]')
api "repos/$repository/rulesets/$main_ruleset_id" \
    | jq --argjson additions "$required_checks" '
        {name, target, enforcement, bypass_actors, conditions, rules}
        | .rules |= map(
            if .type == "required_status_checks" then
                .parameters.required_status_checks =
                    ((.parameters.required_status_checks + $additions) | unique_by(.context))
            else . end
          )
      ' \
    | api --method PUT "repos/$repository/rulesets/$main_ruleset_id" --input - >/dev/null

echo "GitHub release protections applied to $repository."
for environment in npm-prod prod; do
    api "repos/$repository/environments/$environment" \
        --jq '{name, protection_rules, deployment_branch_policy}'
done
api "repos/$repository/rulesets" \
    --jq '.[] | select(.name == "main" or .name == "npm release tags" or .name == "mmcg release tags") | {id, name, target, enforcement}'
