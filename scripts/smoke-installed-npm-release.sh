#!/usr/bin/env bash
# Install the just-published root package from the public npm registry and run
# real revision-bound adapter/signature/import flow plus a two-repository team
# graph. This never consumes local tarballs, so a green result proves registry
# visibility, optional-platform resolution, the native binary, and the shipped
# CLI surface together.

set -euo pipefail

if [ "$#" -ne 1 ] || [ ! -f "$1" ]; then
    echo "usage: $0 <root-package.json>" >&2
    exit 2
fi

MANIFEST="$1"
REGISTRY="https://registry.npmjs.org"
ATTEMPTS="${NPM_INSTALL_SMOKE_ATTEMPTS:-12}"
DELAY="${NPM_INSTALL_SMOKE_DELAY_SECONDS:-10}"
case "$ATTEMPTS" in ''|*[!0-9]*) echo "error: NPM_INSTALL_SMOKE_ATTEMPTS must be a positive integer" >&2; exit 2 ;; esac
case "$DELAY" in ''|*[!0-9]*) echo "error: NPM_INSTALL_SMOKE_DELAY_SECONDS must be a non-negative integer" >&2; exit 2 ;; esac
if [ "$ATTEMPTS" -lt 1 ]; then
    echo "error: NPM_INSTALL_SMOKE_ATTEMPTS must be a positive integer" >&2
    exit 2
fi

read -r PACKAGE_NAME VERSION < <(node - "$MANIFEST" <<'NODE'
const fs = require("node:fs");
const value = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (!/^@[a-z0-9][a-z0-9._-]*\/[a-z0-9][a-z0-9._-]*$/.test(value.name) ||
    !/^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$/.test(value.version)) {
  throw new Error("unsafe package name or version");
}
process.stdout.write(`${value.name} ${value.version}\n`);
NODE
)
SPEC="${PACKAGE_NAME}@${VERSION}"
SMOKE_ROOT=$(mktemp -d)
trap 'rm -rf "$SMOKE_ROOT"' EXIT
NPM_CONFIG_USERCONFIG="$SMOKE_ROOT/npmrc"
export NPM_CONFIG_USERCONFIG
: >"$NPM_CONFIG_USERCONFIG"

attempt=1
while [ "$attempt" -le "$ATTEMPTS" ]; do
    INSTALL_DIR="$SMOKE_ROOT/install-$attempt"
    mkdir "$INSTALL_DIR"
    (
        cd "$INSTALL_DIR"
        npm init -y >/dev/null
        npm install --ignore-scripts --no-save --registry="$REGISTRY" "$SPEC" >install.log 2>&1
    ) || true
    BIN="$INSTALL_DIR/node_modules/.bin/mastermind"
    if [ -x "$BIN" ] && VERSION_OUT=$("$BIN" --version 2>&1) && [ "$VERSION_OUT" = "mastermind $VERSION" ]; then
        "$BIN" facts --help >/dev/null
        "$BIN" team --help >/dev/null
        "$BIN" review export --help >/dev/null

        REPO="$SMOKE_ROOT/repository"
        mkdir -p "$REPO/src"
        printf 'pub fn release_smoke() {}\n' >"$REPO/src/lib.rs"
        printf '%s\n' '{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"release-smoke"}},"results":[{"ruleId":"release.smoke","level":"note","message":{"text":"installed registry smoke"},"locations":[{"physicalLocation":{"artifactLocation":{"uri":"src/lib.rs"},"region":{"startLine":1}}}]}]}]}' >"$REPO/findings.sarif"
        (
            cd "$REPO"
            git init -q
            git config user.email release-smoke@example.invalid
            git config user.name "Release Smoke"
            git add .
            git commit -qm fixture
            "$BIN" index . >/dev/null
            "$BIN" facts adapt \
                --format sarif \
                --input findings.sarif \
                --output facts.json \
                --producer release-smoke \
                --producer-version "$VERSION" \
                --dataset installed-package \
                --root . >/dev/null
        )
        "$BIN" facts keygen \
            --private-key "$REPO/producer.seed" \
            --public-key "$REPO/producer.pub" >"$REPO/keygen.json"
        KEY_ID=$(node -e 'const v=require(process.argv[1]); process.stdout.write(v.key_id)' "$REPO/keygen.json")
        "$BIN" facts sign "$REPO/facts.json" \
            --private-key "$REPO/producer.seed" \
            --signature "$REPO/facts.sig.json" >/dev/null
        "$BIN" facts verify "$REPO/facts.json" \
            --signature "$REPO/facts.sig.json" \
            --public-key "$REPO/producer.pub" \
            --trusted-key-id "$KEY_ID" --json >"$REPO/verify.json"
        "$BIN" --index "$REPO/.mastermind/mmcg.db" enrich \
            --facts "$REPO/facts.json" \
            --signature "$REPO/facts.sig.json" \
            --public-key "$REPO/producer.pub" \
            --trusted-key-id "$KEY_ID" \
            --require-signature >/dev/null
        "$BIN" --index "$REPO/.mastermind/mmcg.db" query facts --top 10 >"$REPO/query.json"

        REPO_TWO="$SMOKE_ROOT/repository-two"
        mkdir -p "$REPO_TWO/src"
        printf 'pub fn team_peer() {}\n' >"$REPO_TWO/src/lib.rs"
        (
            cd "$REPO_TWO"
            git init -q
            git config user.email release-smoke@example.invalid
            git config user.name "Release Smoke"
            git add .
            git commit -qm fixture
            "$BIN" index . >/dev/null
        )
        node - "$SMOKE_ROOT/team.json" "$REPO" "$REPO_TWO" <<'NODE'
const fs = require("node:fs");
const path = require("node:path");
const [output, one, two] = process.argv.slice(2);
fs.writeFileSync(output, JSON.stringify({
  api_version: "mastermind-team/v1",
  repositories: [
    {id: "release-one", root: one, index: path.join(one, ".mastermind/mmcg.db")},
    {id: "release-two", root: two, index: path.join(two, ".mastermind/mmcg.db")}
  ],
  relationships: [{
    id: "release-one-to-two",
    relation: "calls_service",
    from: {repository: "release-one"},
    to: {repository: "release-two"},
    label: "Installed-package team graph smoke"
  }]
}, null, 2) + "\n");
NODE
        "$BIN" team lock "$SMOKE_ROOT/team.json" \
            --output "$SMOKE_ROOT/team.lock.json" >"$SMOKE_ROOT/team-lock-summary.json"
        "$BIN" team map "$SMOKE_ROOT/team.lock.json" >"$SMOKE_ROOT/team-map.json"

        node - "$REPO/facts.json" "$REPO/query.json" "$SMOKE_ROOT/team-lock-summary.json" "$SMOKE_ROOT/team-map.json" "$VERSION" <<'NODE'
const fs = require("node:fs");
const value = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
const query = JSON.parse(fs.readFileSync(process.argv[3], "utf8"));
const teamLock = JSON.parse(fs.readFileSync(process.argv[4], "utf8"));
const teamMap = JSON.parse(fs.readFileSync(process.argv[5], "utf8"));
if (value.api_version !== "mastermind-facts/v1" ||
    value.producer.version !== process.argv[6] ||
    value.facts.length !== 1 ||
    value.facts[0].kind !== "annotation" ||
    query.sources.items[0].signature_status !== "verified" ||
    !/^sha256:[0-9a-f]{64}$/.test(teamLock.manifest_sha256) ||
    teamMap.repositories.returned !== 2 ||
    !teamMap.edges.items.some(edge => edge.kind === "cross_repository")) {
  throw new Error("installed package failed its fact/signature/team contract");
}
NODE
        echo "✓ installed $SPEC from npm and completed index → adapt/sign/import → team-map smoke"
        exit 0
    fi
    if [ "$attempt" -lt "$ATTEMPTS" ]; then
        echo "registry install for $SPEC not ready (attempt $attempt/$ATTEMPTS); retrying"
        sleep "$DELAY"
    fi
    attempt=$((attempt + 1))
done

echo "error: $SPEC never passed the installed-registry smoke" >&2
find "$SMOKE_ROOT" -maxdepth 3 -type f -name install.log -exec tail -n 20 {} \; >&2
exit 1
