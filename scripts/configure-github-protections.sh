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
  - npm-prod required_reviewers: $reviewer
  - npm-prod prevent_self_review: $prevent_self_review
  - npm-prod deployment tag policy: npm-v*
  - active npm release tag ruleset: refs/tags/npm-v*
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

# Configure required_reviewers without reading or replacing environment secrets.
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
}' | api --method PUT "repos/$repository/environments/npm-prod" --input - >/dev/null

if ! api "repos/$repository/environments/npm-prod/deployment-branch-policies" \
    --jq '.branch_policies[]? | select(.name == "npm-v*" and .type == "tag") | .id' \
    | grep -q .; then
    api --method POST "repos/$repository/environments/npm-prod/deployment-branch-policies" \
        -f name='npm-v*' -f type='tag' >/dev/null
fi

npm_ruleset_id=$(api "repos/$repository/rulesets" \
    --jq '.[] | select(.name == "npm release tags" and .target == "tag") | .id' \
    | head -n 1)
npm_ruleset_payload() {
    jq -n '{
      name: "npm release tags",
      target: "tag",
      enforcement: "active",
      bypass_actors: [{actor_id: null, actor_type: "OrganizationAdmin", bypass_mode: "always"}],
      conditions: {ref_name: {include: ["refs/tags/npm-v*"], exclude: []}},
      rules: [
        {type: "deletion"},
        {type: "non_fast_forward"},
        {type: "update"},
        {type: "creation"}
      ]
    }'
}
if [ -n "$npm_ruleset_id" ]; then
    npm_ruleset_payload | api --method PUT "repos/$repository/rulesets/$npm_ruleset_id" --input - >/dev/null
else
    npm_ruleset_payload | api --method POST "repos/$repository/rulesets" --input - >/dev/null
fi

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
api "repos/$repository/environments/npm-prod" \
    --jq '{name, protection_rules, deployment_branch_policy}'
api "repos/$repository/rulesets" \
    --jq '.[] | select(.name == "main" or .name == "npm release tags") | {id, name, target, enforcement}'
