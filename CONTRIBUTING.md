# Contributing

Keep changes focused and include enough evidence to reproduce the result.

## Development setup

Run commands from the repository root. Required tools:

- Rust through `rustup`. It installs the version that [rust-toolchain.toml](rust-toolchain.toml) pins, with Clippy and rustfmt. The crate declares `rust-version = "1.96"` for Cargo users
- Node.js 24 or newer
- Python 3.11 or newer
- [`just`](https://github.com/casey/just) and `cargo-deny`

```bash
just bootstrap
just check
```

`bootstrap` creates the Python virtual environment and installs pinned validator
dependencies. `check` runs formatting, Clippy, Rust tests, repository validation,
npm tests, Lens tests, deterministic eval harnesses and dependency policy checks.
Run the full gate before opening a pull request.

## Focused checks

| Change | Command |
|---|---|
| Rust | `just test` |
| Rust lint and formatting | `just lint` and `just fmt-check` |
| Documentation or repository contracts | `just validate` |
| npm installer | `just npm-test` |
| Native npm packaging | `just npm-smoke-native` |
| Lens frontend | `just lens-ui-test` |
| Eval and evidence harnesses | `just eval-harness` |
| Control-loop characterization | `just eval-control .mastermind/research/control-loop` |
| Index performance | `just benchmark-index` |
| Rust tests on Linux, from committed `HEAD` in Docker | `just test-linux` |
| Release commit | `just release-check`, see [cutting a release](scripts/README.md#cutting-a-release) |

The native npm smoke builds and installs local tarballs in a temporary project.
It does not publish packages. Model-backed evaluations need an authenticated
Claude CLI and run separately with `just evals`. See [evals](evals/README.md) and
[script contracts](scripts/README.md).

Lens and invocation tests bind local ports and manage process groups, so run
`just check` outside any command sandbox. Local Clippy covers only your host
target. CI in `.github/workflows/ci-mmcg.yml` runs the full test suite and an
end-to-end smoke against `tests/ci-fixture/` on six targets: Linux x86_64 and
aarch64 with gnu and musl, macOS aarch64 and Windows x86_64. macOS Intel
(`x86_64-apple-darwin`) is not tested per pull request, so code under
`#[cfg(unix)]` or `#[cfg(windows)]` needs a look at the imports it shares with
other platforms.

The dev profile in `mcp/servers/mmcg/Cargo.toml` keeps `debug = 1`, because
guarded execution pins the Mastermind executable and refuses one over 256 MiB.
Full debug information can push a debug build past that limit.

## Repository layout

| Path | Responsibility |
|---|---|
| `mcp/servers/mmcg/` | Rust CLI, indexer, SQLite store, MCP and Lens backend |
| `mcp/servers/mmcg/assets/lens/` | Lens frontend |
| `skills/`, `agents/` | Installable skill and agent instructions |
| `schemas/` | Versioned JSON contracts |
| `npm/` | npm wrapper and platform packages |
| `action.yml`, `Dockerfile.audit-action` | GitHub Action runtime |
| `docs/` | Guides, architecture and reference |
| `scripts/`, `evals/` | Validation, packaging and evaluation tools |

## Documentation

Human-facing documentation describes current behavior. Each topic has one
owner, and other documents link to it:

| Document | Owns |
|---|---|
| [Getting started](docs/getting-started.md) | Install, `init`, update, build from source and uninstall |
| [Architecture](docs/architecture.md) | Components, data ownership and trust boundaries |
| [Workflow](docs/workflow.md) | The task steps a person follows |
| [CLI and index](docs/reference/mmcg.md) | CLI commands, indexing, language coverage and the workflow audit |
| [MCP](docs/reference/mcp.md) | MCP protocol, write boundary and the tool catalog |
| [Concept search](docs/reference/concept-search.md), [Lens](docs/reference/lens.md), [Policy](docs/reference/policy.md) | Their commands, limits and output |
| [Specs, reports and CI](docs/reference/spec-and-report.md) | Spec fields, executor report rules and the CI gate |
| [Task runtime](docs/reference/task-runtime.md) | Checks, receipts, native executor and reviewer, repair and older task state |
| [Persona](docs/reference/persona.md), [persona context](docs/reference/persona-context.md), [persona evidence](docs/reference/persona-evidence.md) | Profile storage, capture, mining, access, context packets and what the evidence shows |
| Root README and guides | Orientation and task-oriented examples |

- Check examples against the current parser. State the working directory when
  it differs from the repository root.
- Distinguish structural, compiler-resolved, declared and observed evidence.
- Use tables for inputs, outputs, limits and failure behavior. Use a diagram for
  a multi-step flow. Keep rationale only where it explains an operational choice.
- Avoid filler, repeated disclaimers and semicolons in prose. Preserve syntax in
  code samples and the actual boundaries of each claim.
- Measurements need a source, revision, corpus, command, environment and limits.
  See [benchmark methodology](docs/benchmarks.md). A synthetic result is not a
  general accuracy claim.
- Agent/subagent instructions and mirrored templates change together when the
  instruction contract changes. Documentation editing alone must not rewrite them.
- Fixture Markdown is test input, not editorial prose.
- Run `just validate` to check links, artifact mirrors and public contracts.

## Pull requests and releases

Describe the changed behavior, compatibility impact, tests and remaining
verification gaps. Add screenshots when rendered behavior changes. Discuss
breaking CLI, schema or workflow changes before implementation.

Write the commit subject as an imperative sentence that names the outcome, for
example `Refuse spec lines the gate parsers cannot read`.

Publish through the repository release workflows. They retain artifact digests
and approval evidence, then test installation from the public registries.

Report vulnerabilities through [SECURITY.md](SECURITY.md). Other issues can use
the [bug template](.github/ISSUE_TEMPLATE/bug.md). Participation is governed by
the [Code of Conduct](CODE_OF_CONDUCT.md).
