# Contributing

This page is for changing Mastermind itself. Run every command from the
repository root.

## Set up

You need Rust through `rustup`, which installs the version
[rust-toolchain.toml](rust-toolchain.toml) pins with Clippy and rustfmt
(the crate declares `rust-version = "1.96"` for Cargo users), Node.js 24 or
newer, Python 3.11 or newer, [`just`](https://github.com/casey/just) and
`cargo-deny`.

```bash
just bootstrap
just check
```

`bootstrap` creates the Python virtual environment with the pinned validator
dependencies. `check` runs formatting, Clippy, Rust tests, the repository
validator, npm and Lens tests, the deterministic eval harness and the
dependency policy. Run it before you open a pull request, outside any command
sandbox, because Lens and invocation tests bind local ports and manage process
groups.

## Run a focused check

| Change | Command |
|---|---|
| Rust | `just test`, `just lint` and `just fmt-check` |
| Documentation or repository contracts | `just validate`, see [scripts](scripts/README.md) |
| Python tests | `just eval-harness`, see [tests](tests/README.md) |
| npm installer | `just npm-test` |
| Native npm packaging, from local tarballs in a temporary project | `just npm-smoke-native` |
| Lens frontend | `just lens-ui-test` |
| Control-loop characterization | `just eval-control .mastermind/research/control-loop` |
| Index performance | `just benchmark-index`, see [benchmark method](docs/benchmarks.md) |
| Rust tests on Linux, from committed `HEAD` in Docker | `just test-linux` |
| Model-backed evaluations, with an authenticated Claude CLI | `just evals`, see [evals](evals/README.md) |
| Release commit | `just release-check`, see [cutting a release](scripts/README.md#cutting-a-release) |

Local Clippy covers only your host target. CI in `.github/workflows/ci-mmcg.yml`
runs the full suite and an end-to-end smoke against `tests/ci-fixture/` on
Linux x86_64 and aarch64 with gnu and musl, macOS aarch64 and Windows x86_64.
macOS Intel is not tested per pull request. Code under `#[cfg(unix)]` or
`#[cfg(windows)]` needs a look at the imports it shares with other platforms.

The dev profile in `mcp/servers/mmcg/Cargo.toml` keeps `debug = 1`, because
guarded execution refuses a Mastermind executable over 256 MiB and full debug
information can push a debug build past that.

## Find the code

| Path | Responsibility |
|---|---|
| `mcp/servers/mmcg/` | Rust CLI, indexer, SQLite store, MCP and Lens backend |
| `mcp/servers/mmcg/assets/lens/` | Lens frontend |
| `skills/`, `agents/` | Installable skill and agent instructions |
| `schemas/` | Versioned JSON contracts |
| `npm/` | npm wrapper and platform packages |
| `action.yml`, `Dockerfile.audit-action` | GitHub Action runtime |
| `docs/` | Guides and reference, indexed in the [README](README.md#what-you-can-do) |
| `scripts/`, `tests/`, `evals/` | Validation, packaging, tests and evaluation tools |

## Write the docs

Docs describe current behavior. Each topic has one owning document, and other
documents link to it. Check every command and flag against the current CLI, and
use tables for inputs, outputs, limits and failure behavior. History goes in
the [changelog](CHANGELOG.md), and measurements go in retained reports under
`evals/baselines/` with their source, revision, corpus, command, environment
and limits. Agent instructions and their mirrored templates change together,
and only when the instruction contract changes. Fixture Markdown is test input.

## Open a pull request

Describe the changed behavior, compatibility impact, tests and the checks you
did not run, with screenshots when rendered behavior changes. Discuss breaking
CLI, schema or workflow changes before you implement them. Write the commit
subject as an imperative sentence that names the outcome, for example
`Refuse spec lines the gate parsers cannot read`.

Releases go through the repository release workflows in
[cutting a release](scripts/README.md#cutting-a-release). Report
vulnerabilities through [SECURITY.md](SECURITY.md) and other issues with the
[bug template](.github/ISSUE_TEMPLATE/bug.md). Participation follows the
[Code of Conduct](CODE_OF_CONDUCT.md).
