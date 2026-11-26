# Mastermind dev commands. Run `just` to see the list.
#
# Install just: `cargo install just` or `brew install just`.
#
# Recipes are grouped:
#   - mmcg/Rust (build, test, test-linux, lint, package, publish-dry)
#   - repo-wide checks (validate, npm-test, eval-harness, security, check, release-check)
#   - release (bump, release-tags)
#   - dev convenience (install)

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
    @echo "Python venv ready at .venv/. Run 'just check' to verify everything."

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

# Needs Docker and tests committed HEAD only. `git archive` copies it into the
# container's own case-sensitive file system, which a bind mount from macOS would not give.
# Run mmcg tests on Linux in the pinned Rust image (commit first).
test-linux:
    #!/usr/bin/env bash
    set -euo pipefail
    image=$(awk '$1 == "FROM" && $2 ~ /^rust:/ {print $2; exit}' Dockerfile.audit-action)
    # A non-root user, like the CI runners, so permission tests behave the same.
    # ponytail: every run downloads crates and compiles from scratch (~10-15 min); add cache volumes if that wait matters.
    git archive --format=tar HEAD | docker run --rm -i --user "$(id -u):$(id -g)" -e HOME=/tmp/home "$image" \
      bash -euo pipefail -c 'mkdir -p "$HOME" /tmp/src && cd /tmp/src && tar -xf - \
        && rustup toolchain install --no-self-update \
        && cargo test --manifest-path {{MMCG}}/Cargo.toml --locked --all'

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
    {{PY}} -m evals.benchmark.corpus --source-repo .
    {{PY}} -m evals.selection

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
    @echo "Templates synced to {{MMCG}}/templates/. Run 'just validate' to confirm parity."

# Set VERSION in every file that carries it and move [Unreleased] to VERSION.
bump VERSION:
    python3 scripts/release_version.py bump {{VERSION}}

# Expect a long wait: the macOS jobs are the slowest and least predictable part.
# Tag the pushed commit once push CI and both publish dry runs are green. Pushes nothing.
release-tags VERSION:
    #!/usr/bin/env bash
    set -euo pipefail
    repo=xcrft/mastermind
    python3 scripts/release_version.py check
    test "$(python3 -c 'from scripts import release_version as r; print(r.source_version())')" = "{{VERSION}}" \
      || { echo "Cargo.toml is not at {{VERSION}}; run just bump {{VERSION}} first" >&2; exit 1; }
    git fetch --quiet origin main
    sha=$(git rev-parse HEAD)
    test "$sha" = "$(git rev-parse origin/main)" \
      || { echo "HEAD is not origin/main; push the release commit first" >&2; exit 1; }
    # One dry run per commit: running this recipe again waits for the existing
    # run, and a failed one stays failed until it is rerun or fixed forward.
    for workflow in publish-mmcg.yml publish-npm.yml; do
      runs=$(gh run list --repo "$repo" --commit "$sha" --workflow "$workflow" \
        --event workflow_dispatch --limit 1 --json databaseId --jq length)
      if [ "$runs" = 0 ]; then
        gh workflow run "$workflow" --repo "$repo" --ref main
      fi
    done
    git fetch --quiet origin main
    test "$sha" = "$(git rev-parse origin/main)" \
      || { echo "main moved while the dry runs were dispatched; check out the new head and run again" >&2; exit 1; }
    # macOS jobs plus their queue have taken well over an hour.
    export CI_WAIT_SECONDS=${CI_WAIT_SECONDS:-7200}
    bash scripts/require-green-ci.sh "$repo" "$sha"
    bash scripts/require-green-ci.sh "$repo" "$sha" workflow_dispatch publish-mmcg.yml publish-npm.yml
    git tag "mmcg-v{{VERSION}}" "$sha"
    git tag "npm-v{{VERSION}}" "$sha"
    echo "Tagged $sha. Publish with: git push origin mmcg-v{{VERSION}} npm-v{{VERSION}}"

# Run the full deterministic gate before every push.
check: fmt-check lint test validate npm-test lens-ui-test eval-harness security

# The release build and packaging add minutes, so they stay out of `check`.
# Gate a release commit: check, native npm tarball smoke and crates.io publish dry run.
release-check: check npm-smoke-native publish-dry
