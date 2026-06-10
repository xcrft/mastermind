#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

cargo build --release --manifest-path mcp/servers/mmcg/Cargo.toml
python scripts/validate.py
python evals/runner.py --suite auditor --model opus
