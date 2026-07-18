#!/bin/sh
set -eu

fail() {
  printf '%s\n' "mastermind audit action: $1" >&2
  exit 1
}

full_oid() {
  test "${#1}" -eq 40 || return 1
  case "$1" in *[!0-9a-f]*) return 1 ;; esac
}

relative_path() {
  case "$1" in
    ""|/*|*/|*//*|.|..|./*|*/./*|*/.|../*|*/../*|*/..|*\\*) return 1 ;;
    *[![:print:]]*) return 1 ;;
  esac
}

root_path() {
  test "$1" = "." && return 0
  relative_path "$1"
}

workspace=${GITHUB_WORKSPACE:?GITHUB_WORKSPACE is required}
root_input=$(printenv INPUT_ROOT || printf '.')
since=$(printenv INPUT_SINCE || true)
bundle_input=$(printenv 'INPUT_BUNDLE-DIR' || printf '.mastermind/audit-output')
expected_repository=$(printenv 'INPUT_EXPECTED-REPOSITORY' || true)
expected_baseline=$(printenv 'INPUT_EXPECTED-BASELINE' || true)
expected_head=$(printenv 'INPUT_EXPECTED-HEAD' || true)
require_clean=$(printenv 'INPUT_REQUIRE-CLEAN-WORKTREE' || printf true)

full_oid "$since" || fail "since must be a full lowercase commit OID"
full_oid "$expected_baseline" || fail "expected-baseline must be a full lowercase commit OID"
full_oid "$expected_head" || fail "expected-head must be a full lowercase commit OID"
test "$since" = "$expected_baseline" || fail "since and expected-baseline differ"
test "$expected_baseline" != "$expected_head" || fail "baseline and head must differ"
case "$expected_repository" in [A-Za-z0-9._-]*/[A-Za-z0-9._-]*) ;; *) fail "expected-repository must be owner/repo" ;; esac
test "$require_clean" = true || fail "require-clean-worktree must be true"
root_path "$root_input" || fail "root must be a safe repository-relative path or ."
relative_path "$bundle_input" || fail "bundle-dir must be a safe repository-relative path"

workspace_real=$(CDPATH= cd -- "$workspace" && pwd -P) || fail "cannot resolve GITHUB_WORKSPACE"
mkdir -m 700 "$HOME" || fail "cannot create private HOME"
export GIT_CONFIG_NOSYSTEM=1
export GIT_CONFIG_GLOBAL=/dev/null
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=safe.directory
export GIT_CONFIG_VALUE_0=$workspace_real
root="$workspace_real/$root_input"
root_real=$(CDPATH= cd -- "$root" && pwd -P) || fail "cannot resolve root"
case "$root_real/" in "$workspace_real/"*) ;; *) fail "root escapes GITHUB_WORKSPACE" ;; esac

bundle_dir=$(/usr/local/bin/mastermind audit prepare-output --root "$root_real" --path "$bundle_input") || fail "cannot prepare contained bundle-dir"

/usr/local/bin/mastermind ci --since "$since" --root "$root_real" \
  --changed-only --require-executor-report --bundle-dir "$bundle_dir"

aggregate="$bundle_dir/result.json"
tmp="$bundle_dir/.result.tmp"
printf '{"schema_version":1,"verified":[' >"$tmp"
first=true
count=0
for bundle in "$bundle_dir"/*.bundle.json; do
  test -f "$bundle" && test ! -L "$bundle" || fail "no regular audit envelopes produced"
  count=$((count + 1))
  test "$count" -le 256 || fail "too many audit envelopes"
  result="$bundle.verify.json"
  /usr/local/bin/mastermind audit verify "$bundle" \
    --root "$root_real" \
    --expected-repository "$expected_repository" \
    --expected-baseline "$expected_baseline" \
    --expected-head "$expected_head" \
    --json >"$result"
  if "$first"; then first=false; else printf ',' >>"$tmp"; fi
  printf '{"bundle":' >>"$tmp"
  basename "$bundle" | sed 's/\\/\\\\/g; s/"/\\"/g; s/^/"/; s/$/"/' >>"$tmp"
  printf ',"verification":' >>"$tmp"
  tr -d '\n' <"$result" >>"$tmp"
  printf '}' >>"$tmp"
done
test "$count" -gt 0 || fail "no audit envelopes produced"
printf '],"result":"pass"}\n' >>"$tmp"
mv "$tmp" "$aggregate"

delimiter="MMCG_$(od -An -N16 -tx1 /dev/urandom | tr -d ' \n')"
{
  printf 'bundle-dir<<%s\n%s\n%s\n' "$delimiter" "$bundle_dir" "$delimiter"
  printf 'result-json<<%s\n%s\n%s\n' "$delimiter" "$aggregate" "$delimiter"
} >>"${GITHUB_OUTPUT:?GITHUB_OUTPUT is required}"
{
  printf '## Mastermind verifiable audit\n\n'
  printf 'Verified %s sealed schema-v3 envelope(s).\n' "$count"
} >>"${GITHUB_STEP_SUMMARY:?GITHUB_STEP_SUMMARY is required}"
