# Contributing

Keep changes focused and include enough evidence to reproduce the result.

## Development setup

Run commands from the repository root. Required tools:

- Rust 1.96 or newer, as declared in [Cargo.toml](mcp/servers/mmcg/Cargo.toml)
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

The native npm smoke builds and installs local tarballs in a temporary project.
It does not publish packages. Model-backed evaluations require an authenticated
Claude CLI and run separately with `just evals`. See [evals](evals/README.md) and
[script contracts](scripts/README.md).

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

Human-facing documentation describes current behavior. Keep the authority clear:

| Document | Owns |
|---|---|
| [Architecture](docs/architecture.md) | Components, data ownership and trust boundaries |
| [Workflow](docs/workflow.md) | The delivery process and completion gates |
| [Reference](docs/reference/mmcg.md) | CLI/MCP contracts, schemas, limits and precision |
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

Use an imperative commit subject with a conventional prefix, for example
`fix(index): reject stale snapshots`.

Publish through the repository release workflows. They retain artifact digests
and approval evidence, then test installation from the public registries.

Report vulnerabilities through [SECURITY.md](SECURITY.md). Other issues can use
the [bug template](.github/ISSUE_TEMPLATE/bug.md). Participation is governed by
the [Code of Conduct](CODE_OF_CONDUCT.md).
