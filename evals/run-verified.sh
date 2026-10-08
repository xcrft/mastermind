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
python -m unittest discover -s tests -t .
python scripts/test_document_graph.py
python -m unittest evals/test_runner.py evals/test_evidence.py evals/test_benchmark.py evals/test_claude_adapter.py evals/test_benchmark_corpus.py evals/test_benchmark_review.py evals/test_persona_replay.py evals/test_control_loop.py evals/test_hook_intake.py
python -m evals.benchmark_corpus --source-repo .
npm test --prefix npm/mastermind
python evals/runner.py "$@"
