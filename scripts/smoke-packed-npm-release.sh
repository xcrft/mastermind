#!/usr/bin/env bash
# Exercise the exact root and host-platform tarballs before publication.
# Shared by PR CI, release verification, and the local native smoke.
set -euo pipefail

if [ "$#" -ne 3 ] || [ ! -d "$1" ] || [ ! -f "$2" ]; then
    echo "usage: $0 <packed-directory> <root-package.json> <host-variant>" >&2
    exit 2
fi
PACKED_DIR=$(cd "$1" && pwd)
MANIFEST="$2"
VARIANT="$3"
case "$VARIANT" in
    darwin-arm64|darwin-x64|linux-x64-gnu|linux-arm64-gnu|linux-x64-musl|linux-arm64-musl|win32-x64-msvc) ;;
    *) echo "error: unsupported host variant $VARIANT" >&2; exit 2 ;;
esac
VERSION=$(node - "$MANIFEST" <<'NODE'
const fs = require("node:fs");
const value = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
if (value.name !== "@xcraftmind/mastermind" ||
    !/^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$/.test(value.version)) {
  throw new Error("unexpected package identity or version");
}
process.stdout.write(value.version);
NODE
)
ROOT_TGZ="$PACKED_DIR/xcraftmind-mastermind-$VERSION.tgz"
PLATFORM_TGZ="$PACKED_DIR/xcraftmind-mmcg-$VARIANT-$VERSION.tgz"
test -f "$ROOT_TGZ" && test -f "$PLATFORM_TGZ"

SMOKE_ROOT=$(mktemp -d)
trap 'rm -rf "$SMOKE_ROOT"' EXIT
mkdir -p "$SMOKE_ROOT/project/src" "$SMOKE_ROOT/workflows" "$SMOKE_ROOT/compat"
export MASTERMIND_WORKFLOW_HOME="$SMOKE_ROOT/workflows"
cd "$SMOKE_ROOT/project"
npm init -y >/dev/null
npm install --offline --ignore-scripts --no-audit --no-fund --no-save "$ROOT_TGZ" "$PLATFORM_TGZ"
test "$(./node_modules/.bin/mastermind --version)" = "mastermind $VERSION"
./node_modules/.bin/mastermind doctor --json >doctor.json || true
node -e 'const d = require("./doctor.json"); if (!Array.isArray(d.checks)) throw new Error("missing doctor checks")'

# An unowned tarball cannot establish a package-manager update scope.
if ./node_modules/.bin/mastermind update --dry-run --json >manual-update.json; then
    echo "error: unowned tarball accepted for automatic package update" >&2
    exit 1
fi
node -e 'const assert = require("node:assert/strict"); const d = require("./manual-update.json"); assert.equal(d.status, "manual_required"); assert.equal(d.package.install_mode, "unknown")'

# Emulate a direct registry dependency without replacing the candidate bytes.
npm pkg set "devDependencies.@xcraftmind/mastermind=$VERSION"
./node_modules/.bin/mastermind setup claude --project . --write
node <<'NODE'
const assert = require("node:assert/strict");
const fs = require("node:fs");
const entry = JSON.parse(fs.readFileSync(".mcp.json", "utf8")).mcpServers.mmcg;
assert.deepEqual(entry, {
  command: process.execPath,
  args: [fs.realpathSync("node_modules/@xcraftmind/mastermind/bin/mastermind.js"), "serve"],
});
NODE
./node_modules/.bin/mastermind update --dry-run --json >project-update.json
node <<'NODE'
const assert = require("node:assert/strict");
const d = require("./project-update.json");
assert.equal(d.status, "planned");
assert.equal(d.package.install_mode, "project");
assert.equal(d.package.command.cwd, process.cwd());
assert.ok(d.package.command.argv.includes("--save-dev"));
NODE

./node_modules/.bin/mmcg setup claude --project "$SMOKE_ROOT/compat" --write
node - "$SMOKE_ROOT/compat/.mcp.json" "@xcraftmind/mmcg-$VARIANT" <<'NODE'
const assert = require("node:assert/strict");
const fs = require("node:fs");
const entry = JSON.parse(fs.readFileSync(process.argv[2], "utf8")).mcpServers.mmcg;
const executable = process.platform === "win32" ? "mmcg.exe" : "mmcg";
const binary = fs.realpathSync(require.resolve(`${process.argv[3]}/bin/${executable}`));
assert.deepEqual(entry, { command: binary, args: ["serve"] });
NODE

printf 'pub fn release_smoke() {}\n' >src/lib.rs
./node_modules/.bin/mastermind init --client none --json >"$SMOKE_ROOT/init.json"
node - "$SMOKE_ROOT/init.json" <<'NODE'
const assert = require("node:assert/strict");
const fs = require("node:fs");
const result = JSON.parse(fs.readFileSync(process.argv[2], "utf8"));
assert.equal(result.status, "configured");
assert.equal(result.settings.mining, "off");
assert.deepEqual(result.settings.clients, []);
assert.ok(fs.statSync(".mastermind/mmcg.db").isFile());
assert.ok(fs.statSync("CONTEXT.md").isFile());
NODE

./node_modules/.bin/mastermind update --workflow-only --client all
./node_modules/.bin/mastermind doctor --workflow --client all --json >workflow-doctor.json
node <<'NODE'
const assert = require("node:assert/strict");
const d = require("./workflow-doctor.json");
assert.deepEqual(d.clients.map(row => row.client), ["claude", "codex"]);
assert.ok(d.clients.every(row => row.parity));
NODE
test -f "$MASTERMIND_WORKFLOW_HOME/.claude/agents/mastermind-auditor.md"
test -f "$MASTERMIND_WORKFLOW_HOME/.codex/skills/mastermind-project-map/SKILL.md"
test ! -d "$MASTERMIND_WORKFLOW_HOME/.codex/agents"
printf '%s\n' '{"mcpServers":{"mmcg":{"command":"mastermind","args":["serve"]}}}' >"$MASTERMIND_WORKFLOW_HOME/.mcp.json"
for PROFILE in core frontend security full; do
    ./node_modules/.bin/mastermind update --workflow-only --client all --profile "$PROFILE"
    for CLIENT_ROOT in .claude .codex; do
        ./node_modules/.bin/mastermind workflow audit --root "$MASTERMIND_WORKFLOW_HOME/$CLIENT_ROOT" --json >workflow-audit.json
        node - "$PROFILE" <<'NODE'
const assert = require("node:assert/strict");
const d = require("./workflow-audit.json");
assert.equal(d.schema_version, 1);
assert.equal(d.complete, true);
assert.equal(d.profile, process.argv[2]);
assert.deepEqual(d.diagnostics.filter(item => item.severity === "error"), []);
NODE
    done
done
echo "Packed npm smoke passed for $VARIANT ($VERSION): setup, index, update scope, and four workflow profiles"
