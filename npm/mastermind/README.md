# @xcraftmind/mastermind

[![npm](https://img.shields.io/badge/npm-v2.0.0-CB3837?logo=npm)](https://www.npmjs.com/package/@xcraftmind/mastermind)
[![CI](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg)](https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml)
[![license: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/xcrft/mastermind/blob/main/LICENSE)

Local codegraph and evidence-backed architecture review for AI coding agents.

Mastermind indexes a repository into SQLite, connects a Git diff to affected
callers, tests, components, policies, and external evidence, and exposes the
same bounded snapshot through CLI, MCP, Lens, SARIF, and a standalone review
package.

## Install

Requires Node.js 24 or newer. The package selects a prebuilt native binary for
macOS, Linux, or Windows; Rust is not required.

```bash
npm install -g @xcraftmind/mastermind
mastermind --version
```

## First review

```bash
cd your-repository
mastermind index .
mastermind impact --since main
mastermind ui --since main
```

`index` writes `.mastermind/mmcg.db`. The Lens server binds to loopback, reads
the index without mutating it, and loads no remote frontend resources.

Export a review that works without an installed binary:

```bash
mastermind review export --since main --out mastermind-review
```

The output contains standalone HTML, SARIF, a short Markdown summary, a
revision/evidence manifest, and a pinned GitHub Actions workflow.

## AI client setup

```bash
mastermind install --client all
mastermind setup cursor --scope user --write
mastermind setup continue --scope user --write
mastermind doctor --workflow --client all
```

Setup commands preview by default and write only with `--write`. Mastermind
supports Claude Code, Codex, Cursor, Continue, and generic MCP stdio clients.
The MCP server exposes 28 bounded tools: 27 read-only queries and one additive
write to the local gitignored scratchpad.

## Capabilities

- component maps, entry points, hotspots, centrality, and dependency cycles;
- Git-aware changed symbols, blast radius, API drift, and candidate tests;
- base-versus-head architecture, ownership, cycle, and hotspot drift;
- architecture policy checks with text, JSON, or SARIF output;
- SARIF, LCOV/Cobertura, JUnit, OpenTelemetry, CODEOWNERS, churn, and project
  decision overlays;
- optional compiler-resolved SCIP definitions and references;
- revision-bound declarative facts with optional Ed25519 provenance;
- bounded, pinned maps across several local repositories;
- Direct, Verified, and Strict spec-driven workflow modes.

Supported languages: Python, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust,
C#, Go, Java, PHP, and C/C++.

Supported binaries: macOS arm64/x64; Linux glibc and musl arm64/x64; Windows
x64.

## Evidence model

The default Tree-sitter graph is syntactic and toolchain-free. Optional SCIP
facts are compiler-resolved; imported runtime relationships are observed;
team-manifest relationships are declared. Mastermind keeps that provenance
visible and does not turn external evidence into unproven graph topology.

Dynamic dispatch, reflection, generated code, dependency injection, re-exports,
overloads, and cross-language calls may be incomplete. Candidate tests and
unreferenced symbols are review inputs, not permission to skip tests or delete
code. Stale indexes, collisions, work limits, and truncation are explicit.

Deterministic indexing, MCP, Lens, policy checks, fact ingestion, and review
export are local. Explicit agent-assisted initialization and deep style mining
can send repository content through the configured AI client.

## Performance

On the published synthetic Rust benchmark, an Apple M3 Pro indexed 1,000 files
and 20,000 functions in a 310 ms median cold run; an unchanged scan took 41 ms.
A 10,000-file, 200,000-function corpus took 3.20 s cold and 353 ms unchanged.

These are release-mode medians on one machine, not portable guarantees or
competitor comparisons. See the
[full methodology, ranges, and reproduction command](https://github.com/xcrft/mastermind/blob/main/docs/benchmarks.md).

## Documentation

- [Getting started](https://github.com/xcrft/mastermind/blob/main/docs/getting-started.md)
- [Client integrations](https://github.com/xcrft/mastermind/tree/main/docs/integrations)
- [CLI and MCP reference](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md)
- [Workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md)
- [Fact-ingestion SDK](https://github.com/xcrft/mastermind/blob/main/docs/fact-ingestion-sdk.md)
- [Benchmarks](https://github.com/xcrft/mastermind/blob/main/docs/benchmarks.md)

The same binary is available from crates.io as
[`mmcg`](https://crates.io/crates/mmcg). npm installs it as both `mastermind`
and `mmcg`.

## License

MIT — [source and license](https://github.com/xcrft/mastermind).
