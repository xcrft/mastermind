#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

cargo fmt --check --manifest-path mcp/servers/mmcg/Cargo.toml
cargo clippy --locked --all-targets --manifest-path mcp/servers/mmcg/Cargo.toml -- -D warnings
cargo test --locked --all --manifest-path mcp/servers/mmcg/Cargo.toml
cargo build --release --locked --manifest-path mcp/servers/mmcg/Cargo.toml
cargo deny --manifest-path mcp/servers/mmcg/Cargo.toml check
python scripts/validate.py
python scripts/test_audit_workflow_security.py
python -m unittest evals/test_runner.py
npm test --prefix npm/mastermind
python evals/runner.py "$@"
