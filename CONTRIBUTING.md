# Contributing

Ship one focused change, make its evidence easy to replay, and leave unrelated
local state alone. Mastermind combines a Rust codegraph engine, a browser review
surface, and installable workflow contracts, so a small edit can cross more
than one distribution boundary.

## Repository layout

| Path | Responsibility |
|---|---|
| `mcp/servers/mmcg/` | Rust CLI, indexer, SQLite store, MCP server, Lens backend |
| `mcp/servers/mmcg/assets/lens/` | Static Lens application |
| `skills/`, `agents/` | Installed workflow and agent contracts |
| `schemas/` | Public, versioned JSON contracts |
| `npm/` | npm wrapper and platform packages |
| `action.yml`, `Dockerfile.audit-action` | GitHub Action runtime |
| `docs/` | User and maintainer documentation |
| `scripts/` | Validation, packaging, release, and smoke-test tooling |
| `evals/` | Deterministic harness tests and optional model-backed evaluations |

## Prerequisites

- Rust toolchain declared in `rust-toolchain.toml`
- Node.js 24 or newer
- Python 3.11 or newer
- [`just`](https://github.com/casey/just)
- `cargo-deny`

Install the Python validator dependencies once:

```bash
python3 -m venv .venv
.venv/bin/pip install --require-hashes -r scripts/requirements.txt
```

## The ship gate

Run this before opening a pull request:

```bash
just check
```

`just check` runs the locked Rust tests, formatting and Clippy checks, npm
tests, Lens tests, workflow-security tests, deterministic eval-harness tests,
repository validation, and `cargo deny` policy checks. A passing subset is
useful while developing but does not replace this gate.

## Focused checks

| Change | Fast local command |
|---|---|
| Rust implementation | `cargo test --manifest-path mcp/servers/mmcg/Cargo.toml --locked` |
| Rust lint | `cargo clippy --manifest-path mcp/servers/mmcg/Cargo.toml --locked --all-targets --all-features -- -D warnings` |
| Rust formatting | `cargo fmt --manifest-path mcp/servers/mmcg/Cargo.toml --all -- --check` |
| Public docs or repository contracts | `.venv/bin/python scripts/validate.py` |
| npm wrapper or packaging | `just npm-smoke-native` |
| Lens frontend | `node --test mcp/servers/mmcg/assets/lens/app.test.cjs` |
| Eval harness | `.venv/bin/python -m unittest evals/test_runner.py` |

`just npm-smoke-native` builds and installs local tarballs in a temporary
project. It never reads from or publishes to npm.

Model-backed evals require an authenticated `claude` CLI and are intentionally
not part of ordinary CI:

```bash
bash evals/run-verified.sh
```

See [evals/README.md](evals/README.md) for suites and limitations.

## Documentation changes

- Lead with the outcome, then give the shortest copyable path to it.
- Put task-oriented guides in `docs/`; keep the root README as the product and
  onboarding page.
- Put exhaustive CLI and MCP details in
  [docs/reference/mmcg.md](docs/reference/mmcg.md).
- Use commands that run from the repository root unless a section says
  otherwise.
- Label syntactic, compiler-resolved, declared, and observed evidence
  separately.
- Delete filler and narration. Keep rationale, trust boundaries, failure modes,
  and the facts a reader cannot recover from the command itself.
- Do not publish speed or accuracy comparisons without a reproducible corpus,
  command, environment, and correctness boundary.
- Run `.venv/bin/python scripts/validate.py`; it checks internal links, mirrors,
  versioned artifacts, tool documentation, and release contracts.

Benchmark methodology and current reference measurements live in
[docs/benchmarks.md](docs/benchmarks.md). Benchmark changes must record the
fixture size, command, number of runs, machine class, toolchain, and range—not
only the fastest sample.

## Pull requests

Include:

1. the behavior or contract changed;
2. the affected files and deliberate exclusions;
3. the commands run and their results;
4. any unavailable live, registry, browser, model, or authorization proof;
5. screenshots only when rendered behavior changed.

Use an imperative commit subject with a conventional prefix, for example
`fix(index): reject stale snapshots`. Discuss compatibility-breaking CLI,
schema, or workflow changes in an issue before implementation.

## Releases

Do not publish from a local checkout. The repository workflows build and carry
forward exact artifacts, checksums, and approval evidence. Release smoke tests
install the public registry package rather than reusing workspace artifacts.

## Security

Report vulnerabilities privately as described in [SECURITY.md](SECURITY.md).
For other bugs, use the
[bug report template](.github/ISSUE_TEMPLATE/bug.md) and include
`mastermind --version`, the operating system, expected behavior, and the exact
failure.

All participation is governed by the
[Code of Conduct](CODE_OF_CONDUCT.md).
