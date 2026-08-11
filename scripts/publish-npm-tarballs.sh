#!/usr/bin/env bash
# Resume-safe npm publication for an assembled root package and its platform
# packages. Existing versions are skipped only when the registry's SHA-512
# integrity is exactly the integrity of the verified local tarball.

set -euo pipefail

if [ "$#" -ne 2 ]; then
    echo "usage: $0 <packed-directory> <root-package.json>" >&2
    exit 2
fi

PACKED_DIR="$1"
ROOT_MANIFEST="$2"

if [ ! -d "$PACKED_DIR" ] || [ ! -f "$ROOT_MANIFEST" ]; then
    echo "error: packed directory and root package manifest must exist" >&2
    exit 2
fi

VERIFY_ATTEMPTS="${NPM_PUBLISH_VERIFY_ATTEMPTS:-6}"
VERIFY_DELAY="${NPM_PUBLISH_VERIFY_DELAY_SECONDS:-2}"
NPM_REGISTRY="https://registry.npmjs.org"
case "$VERIFY_ATTEMPTS" in ''|*[!0-9]*) echo "error: NPM_PUBLISH_VERIFY_ATTEMPTS must be a positive integer" >&2; exit 2 ;; esac
case "$VERIFY_DELAY" in ''|*[!0-9]*) echo "error: NPM_PUBLISH_VERIFY_DELAY_SECONDS must be a non-negative integer" >&2; exit 2 ;; esac
if [ "$VERIFY_ATTEMPTS" -lt 1 ]; then
    echo "error: NPM_PUBLISH_VERIFY_ATTEMPTS must be a positive integer" >&2
    exit 2
fi

TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT
RECORDS="$TMP_DIR/packages.tsv"

node - "$ROOT_MANIFEST" >"$RECORDS" <<'NODE'
const fs = require("node:fs");
const manifestPath = process.argv[2];
const root = JSON.parse(fs.readFileSync(manifestPath, "utf8"));
const validName = /^(?:@[a-z0-9][a-z0-9._-]*\/)?[a-z0-9][a-z0-9._-]*$/;
const validVersion = /^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$/;
function record(kind, name, version) {
  if (!validName.test(name) || !validVersion.test(version)) {
    throw new Error(`unsafe package identity: ${name}@${version}`);
  }
  const filename = `${name.replace(/^@/, "").replaceAll("/", "-")}-${version}.tgz`;
  process.stdout.write(`${kind}\t${name}@${version}\t${filename}\n`);
}
for (const [name, version] of Object.entries(root.optionalDependencies || {})) {
  if (version !== root.version) {
    throw new Error(`${name} is ${version}; root package is ${root.version}`);
  }
  record("platform", name, version);
}
record("root", root.name, root.version);
NODE

expected_count=$(wc -l <"$RECORDS" | tr -d ' ')
actual_count=$(find "$PACKED_DIR" -maxdepth 1 -type f -name '*.tgz' | wc -l | tr -d ' ')
if [ "$actual_count" != "$expected_count" ]; then
    echo "error: expected $expected_count release tarballs, found $actual_count" >&2
    exit 1
fi

while IFS=$'\t' read -r kind spec filename; do
    test -n "$kind" && test -n "$spec" && test -n "$filename" || {
        echo "error: invalid package publication record" >&2
        exit 1
    }
    tarball="$PACKED_DIR/$filename"
    if [ ! -f "$tarball" ] || [ -L "$tarball" ] || [ ! -s "$tarball" ]; then
        echo "error: expected regular non-empty tarball: $tarball" >&2
        exit 1
    fi
done <"$RECORDS"

local_integrity() {
    node - "$1" <<'NODE'
const crypto = require("node:crypto");
const fs = require("node:fs");
const bytes = fs.readFileSync(process.argv[2]);
process.stdout.write(`sha512-${crypto.createHash("sha512").update(bytes).digest("base64")}`);
NODE
}

# Prints the registry integrity. Return 44 only for a confirmed 404; every
# authentication, transport, and parsing failure stays fatal.
registry_integrity() {
    local spec="$1"
    local error_file="$TMP_DIR/npm-view-error"
    local output parsed
    if output=$(npm view "$spec" dist.integrity --json --registry="$NPM_REGISTRY" 2>"$error_file"); then
        if ! parsed=$(node -e '
const value = JSON.parse(process.argv[1]);
if (typeof value !== "string" || !value.startsWith("sha512-")) process.exit(1);
process.stdout.write(value);
' "$output"); then
            echo "error: registry returned invalid integrity metadata for $spec" >&2
            return 1
        fi
        printf '%s\n' "$parsed"
        return 0
    fi
    if grep -Eiq '(^|[[:space:]])E404([[:space:]]|$)|404 Not Found' "$error_file"; then
        return 44
    fi
    echo "error: registry lookup failed for $spec" >&2
    sed -n '1,5p' "$error_file" >&2
    return 1
}

PLAN="$TMP_DIR/publish-plan.tsv"
: >"$PLAN"

# Complete every immutable registry lookup before the first publish. A version
# collision anywhere, including at the root, must abort without creating a
# wider partial release.
while IFS=$'\t' read -r kind spec filename; do
    tarball="$PACKED_DIR/$filename"
    expected=$(local_integrity "$tarball")
    if remote=$(registry_integrity "$spec"); then
        if [ "$remote" != "$expected" ]; then
            echo "error: integrity mismatch for already-published $spec" >&2
            echo "  local:    $expected" >&2
            echo "  registry: $remote" >&2
            exit 1
        fi
        state=present
    else
        status=$?
        if [ "$status" -ne 44 ]; then
            exit "$status"
        fi
        state=missing
    fi
    printf '%s\t%s\t%s\t%s\t%s\n' "$kind" "$spec" "$filename" "$expected" "$state" >>"$PLAN"
done <"$RECORDS"

publish_missing() {
    local spec="$1"
    local tarball="$2"
    local expected="$3"
    local remote status attempt

    echo "→ npm publish $spec"
    npm publish --provenance --access public --registry="$NPM_REGISTRY" "$tarball"

    attempt=1
    while [ "$attempt" -le "$VERIFY_ATTEMPTS" ]; do
        if remote=$(registry_integrity "$spec"); then
            if [ "$remote" != "$expected" ]; then
                echo "error: registry integrity mismatch after publishing $spec" >&2
                return 1
            fi
            echo "✓ $spec published with matching integrity"
            return 0
        else
            status=$?
            if [ "$status" -ne 44 ]; then
                return "$status"
            fi
        fi
        if [ "$attempt" -lt "$VERIFY_ATTEMPTS" ]; then
            sleep "$VERIFY_DELAY"
        fi
        attempt=$((attempt + 1))
    done
    echo "error: $spec was accepted but did not become visible after $VERIFY_ATTEMPTS checks" >&2
    return 1
}

# Platforms are independently resumable. The root is reached only after every
# platform has either been published now or preflighted byte-for-byte.
while IFS=$'\t' read -r kind spec filename expected state; do
    if [ "$kind" = platform ]; then
        if [ "$state" = present ]; then
            echo "= $spec already published with matching integrity"
        else
            publish_missing "$spec" "$PACKED_DIR/$filename" "$expected"
        fi
    fi
done <"$PLAN"

while IFS=$'\t' read -r kind spec filename expected state; do
    if [ "$kind" = root ]; then
        if [ "$state" = present ]; then
            echo "= $spec already published with matching integrity"
        else
            publish_missing "$spec" "$PACKED_DIR/$filename" "$expected"
        fi
    fi
done <"$PLAN"
