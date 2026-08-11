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
    install_dir="$smoke_root/install"
    mkdir -p "$pack_dir" "$install_dir"
    (cd npm/mastermind && npm pack --pack-destination "$pack_dir")
    (cd "npm/platforms/$variant" && npm pack --pack-destination "$pack_dir")

    version=$(node -p "require('./npm/mastermind/package.json').version")
    root_tgz="$pack_dir/xcraftmind-mastermind-${version}.tgz"
    platform_tgz="$pack_dir/xcraftmind-mmcg-${variant}-${version}.tgz"
    test -f "$root_tgz" && test -f "$platform_tgz"
    cd "$install_dir"
    npm init -y >/dev/null
    npm install --no-save --offline "$root_tgz" "$platform_tgz"
    actual=$(./node_modules/.bin/mastermind --version)
    test "$actual" = "mastermind $version" || {
        echo "wrapper version mismatch: expected mastermind $version, got $actual" >&2
        exit 1
    }
    ./node_modules/.bin/mastermind doctor --json >doctor.json || true
    python3 -c 'import json; value=json.load(open("doctor.json")); assert "checks" in value, value'
    echo "Native npm tarball smoke passed for $target ($version)."

# Test audit publication fixtures and the eval harness itself without calling a model.
eval-harness:
    {{PY}} scripts/test_audit_workflow_security.py
    {{PY}} -m unittest evals/test_runner.py

# Enforce RustSec, license, duplicate, wildcard, and source policy.
security:
    cargo deny --manifest-path {{MMCG}}/Cargo.toml check

# Run all model-backed behavioral eval suites. Extra args are forwarded to the runner.
evals *ARGS:
    bash evals/run-verified.sh {{ARGS}}

# Copy canonical CLAUDE.md / spec template into the mmcg crate mirror.
# Run after editing the canonical files — otherwise `cargo publish` ships a stale mmcg init.
sync-templates:
    cp agents/claude-md/mastermind-context.md {{MMCG}}/templates/context.md
    cp agents/claude-md/mastermind-workflow.md {{MMCG}}/templates/workflow.md
    @echo "Templates synced to {{MMCG}}/templates/. Run `just validate` to confirm parity."

# Everything deterministic that should pass before pushing.
check: fmt-check lint test validate npm-test eval-harness security

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

# End-to-end smoke: scratch dir → mmcg init → mmcg index → outline a file.
smoke:
    #!/usr/bin/env bash
    set -euo pipefail
    smoke_dir="$(mktemp -d)"
    trap 'rm -rf "$smoke_dir"' EXIT
    mkdir -p "$smoke_dir/src"
    cp {{MMCG}}/src/queries.rs "$smoke_dir/src/queries.rs"
    {{MMCG}}/target/debug/mmcg init "$smoke_dir" --no-claude --no-global --no-seed-style
    {{MMCG}}/target/debug/mmcg --index "$smoke_dir/.mastermind/mmcg.db" query outline src/queries.rs | head -20
    echo "Smoke passed."
