# Mastermind dev commands. Run `just` to see the list.
#
# Install just: `cargo install just` or `brew install just`.
#
# Recipes are grouped:
#   - mmcg/Rust (build, test, lint, package, publish-dry)
#   - repo-wide checks (validate, npm-test, eval-harness, security, check)
#   - dev convenience (install, smoke, index, outline)

# Path to the mmcg crate — referenced via `--manifest-path` so recipes can run from anywhere.
MMCG := "mcp/servers/mmcg"

# Python venv that hosts the validator deps. Created on first `just bootstrap`.
PY := ".venv/bin/python"

# ---- default ----

# Show all recipes.
default:
    @just --list

# ---- one-time setup ----

# Create the Python venv used by the validator and deterministic Python tests.
bootstrap:
    python3 -m venv .venv
    {{PY}} -m pip install -q --upgrade pip
    {{PY}} -m pip install -q --require-hashes -r scripts/requirements.txt
    @echo "Python venv ready at .venv/. Run `just check` to verify everything."

# ---- mmcg (Rust) ----

# Build mmcg debug binary.
build:
    cargo build --manifest-path {{MMCG}}/Cargo.toml --locked

# Build mmcg release binary (smaller, faster — used for distribution).
build-release:
    cargo build --release --manifest-path {{MMCG}}/Cargo.toml --locked

# Run mmcg unit tests.
test:
    cargo test --manifest-path {{MMCG}}/Cargo.toml --locked --all

# Run clippy with warnings-as-errors. Matches what CI should enforce.
lint:
    cargo clippy --manifest-path {{MMCG}}/Cargo.toml --locked --all-targets -- -D warnings

# Apply rustfmt.
fmt:
    cargo fmt --manifest-path {{MMCG}}/Cargo.toml

# Verify rustfmt would not change anything.
fmt-check:
    cargo fmt --manifest-path {{MMCG}}/Cargo.toml --check

# Install mmcg into ~/.cargo/bin (local dev install).
install:
    cargo install --path {{MMCG}} --locked

# Package the crate — verifies the tarball that would ship to crates.io builds clean.
package:
    cargo package --manifest-path {{MMCG}}/Cargo.toml --locked --allow-dirty

# Dry-run publish — connects to crates.io but does not upload.
publish-dry:
    cargo publish --manifest-path {{MMCG}}/Cargo.toml --locked --dry-run --allow-dirty

# Measure cold, warm, and incremental indexing with peak process RSS.
benchmark-index:
    cargo bench --manifest-path {{MMCG}}/Cargo.toml --locked --bench indexer

# Clean Rust build artifacts.
clean:
    cargo clean --manifest-path {{MMCG}}/Cargo.toml

# ---- repo-wide ----

# Run the artifact validator (frontmatter, slugs, wikilinks, relative links, mmcg template-mirror parity).
validate:
    {{PY}} scripts/validate.py

# Test the cross-client workflow installer and ownership manifest.
npm-test:
    npm test --prefix npm/mastermind

# Exercise the dependency-free Lens DOM harness and accessibility contracts.
lens-ui-test:
    node --test mcp/servers/mmcg/assets/lens/app.test.cjs

# Exercise the local host's complete distribution chain without a registry:
# native release build -> platform package -> npm pack -> tarball install -> wrapper smoke.
npm-smoke-native:
    #!/usr/bin/env bash
    set -euo pipefail
    target=$(rustc -vV | awk '/^host: / {print $2}')
    case "$target" in
        aarch64-apple-darwin) variant=darwin-arm64; executable=mmcg ;;
        x86_64-apple-darwin) variant=darwin-x64; executable=mmcg ;;
        x86_64-unknown-linux-gnu) variant=linux-x64-gnu; executable=mmcg ;;
        aarch64-unknown-linux-gnu) variant=linux-arm64-gnu; executable=mmcg ;;
        x86_64-unknown-linux-musl) variant=linux-x64-musl; executable=mmcg ;;
        aarch64-unknown-linux-musl) variant=linux-arm64-musl; executable=mmcg ;;
        x86_64-pc-windows-msvc) variant=win32-x64-msvc; executable=mmcg.exe ;;
        *) echo "unsupported native npm smoke target: $target" >&2; exit 2 ;;
    esac
    cargo build --release --manifest-path {{MMCG}}/Cargo.toml --locked
    ./scripts/build-npm-packages.sh "$target" "{{MMCG}}/target/release/$executable"
    bash scripts/stage-npm-share.sh

    smoke_root=$(mktemp -d)
    trap 'rm -rf "$smoke_root"' EXIT
    pack_dir="$smoke_root/packed"
    mkdir -p "$pack_dir"
    (cd npm/mastermind && npm pack --pack-destination "$pack_dir")
    (cd "npm/platforms/$variant" && npm pack --pack-destination "$pack_dir")

    bash scripts/smoke-packed-npm-release.sh "$pack_dir" npm/mastermind/package.json "$variant"

# Test audit publication, document snapshots, and eval harnesses without a model or build.
eval-harness:
    {{PY}} -m unittest discover -s tests -t .

# Check completion invariants and sampled CLI behavior without model calls.
eval-control output:
    {{PY}} -m evals.control --output {{quote(output)}}

# Enforce RustSec, license, duplicate, wildcard, and source policy.
security:
    cargo deny --manifest-path {{MMCG}}/Cargo.toml check

# Run all model-backed behavioral eval suites. Extra args are forwarded to the runner.
evals *ARGS:
    bash evals/run-verified.sh {{ARGS}}

# Sync project context and workflow templates shipped by the Cargo package.
sync-templates:
    cp agents/claude-md/mastermind-context.md {{MMCG}}/templates/context.md
    cp agents/claude-md/mastermind-workflow.md {{MMCG}}/templates/workflow.md
    @echo "Templates synced to {{MMCG}}/templates/. Run `just validate` to confirm parity."

# Everything deterministic that should pass before pushing.
check: fmt-check lint test validate npm-test lens-ui-test eval-harness security

# ---- dev smoke / one-shot queries ----

# Index this repo into a scratch DB and print stats.
index:
    {{MMCG}}/target/debug/mmcg --index /tmp/mmcg-mastermind.db index .

# Print the symbol tree of a file via the local mmcg build.
# Usage: just outline mcp/servers/mmcg/src/queries.rs
outline FILE:
    {{MMCG}}/target/debug/mmcg --index /tmp/mmcg-mastermind.db query outline {{FILE}}

# What's been re-indexed recently. Usage: just recent 2h
recent SINCE="1h":
    {{MMCG}}/target/debug/mmcg --index /tmp/mmcg-mastermind.db query recent --since {{SINCE}}

# Exercise local onboarding and indexing in a disposable project.
smoke:
    #!/usr/bin/env bash
    set -euo pipefail
    smoke_dir="$(mktemp -d)"
    trap 'rm -rf "$smoke_dir"' EXIT
    mkdir -p "$smoke_dir/src"
    cp tests/ci-fixture/src/lib.py "$smoke_dir/src/lib.py"
    git -C "$smoke_dir" -c core.hooksPath=/dev/null init --quiet --initial-branch=smoke
    {{MMCG}}/target/debug/mmcg init "$smoke_dir" --client none --workflow off
    {{MMCG}}/target/debug/mmcg --index "$smoke_dir/.mastermind/mmcg.db" query outline src/lib.py > "$smoke_dir/outline.json"
    {{PY}} - "$smoke_dir/outline.json" <<'PY'
    import json, sys
    with open(sys.argv[1]) as stream:
        outline = json.load(stream)
    assert outline["file"] == "src/lib.py", outline
    functions = {node["name"] for node in outline["nodes"][0]["children"] if node["kind"] == "function"}
    assert functions == {"greet", "caller"}, outline
    print("Smoke passed: greet and caller indexed.")
    PY
