<p align="center">
  <img src="https://raw.githubusercontent.com/xcrft/mastermind/main/docs/assets/brand/mastermind-mark.svg" alt="Mastermind logo" width="88">
</p>

<h1 align="center">@xcraftmind/mastermind</h1>

<p align="center">
  <strong>Review the blast radius, not just the patch.</strong>
</p>

<p align="center">
  Local codegraph and evidence-backed architecture review for developers and coding agents—without uploading the repository.
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@xcraftmind/mastermind"><img src="https://img.shields.io/badge/npm-v2.0.1-CB3837?logo=npm" alt="npm version 2.0.1"></a>
  <a href="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml"><img src="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg" alt="CI status"></a>
  <a href="https://github.com/xcrft/mastermind/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-4f46e5.svg" alt="MIT license"></a>
</p>

<p align="center">
  <img src="https://raw.githubusercontent.com/xcrft/mastermind/main/docs/assets/brand/mastermind-hero.webp" alt="A bounded code graph flowing through the Mastermind lens from change to impact and test evidence" width="900">
</p>

## See the consequences behind the diff

A diff tells you what changed. Mastermind shows what the change reaches:
downstream callers, architecture boundaries, candidate tests, ownership,
security findings, runtime evidence, and repository policy.

One local snapshot powers the CLI, 28 bounded MCP tools, the read-only Lens UI,
SARIF output, and a standalone review package.

## Your first review in three commands

Requires Node.js 24+. The package selects a prebuilt native binary for macOS,
Linux, or Windows. Rust is not required.

```bash
npm install -g @xcraftmind/mastermind

cd your-repository
mastermind index .
mastermind impact --since main
mastermind ui --since main
```

`index` writes a local graph to `.mastermind/mmcg.db`. Lens binds to loopback,
reads the index without mutating it, and loads no remote frontend resources.

<p align="center">
  <img src="https://raw.githubusercontent.com/xcrft/mastermind/main/docs/images/lens/mastermind-lens-live-desktop.png" alt="Mastermind Lens showing changed symbols, downstream impact, boundary crossings, test candidates, and explicit partial evidence" width="900">
</p>

## One graph, five useful surfaces

| Job | Surface |
|---|---|
| Review a branch | Changed symbols → downstream reach → boundary crossings → candidate tests |
| Audit a codebase | Components, entry points, cycles, centrality, ownership concentration, and hotspots |
| Guard architecture | Policy checks with text, JSON, and SARIF output |
| Ground an agent | Bounded MCP queries over the same local graph |
| Share the result | Standalone offline Lens, SARIF, summary, and revision/evidence manifest |

Mastermind keeps uncertainty visible. Stale indexes, repository drift, work
limits, truncation, unavailable analysis, and partial evidence are result
states, not footnotes hidden behind a clean badge.

## Bring your existing evidence

```bash
mastermind ui --since main \
  --sarif semgrep.sarif --sarif codeql.sarif \
  --coverage lcov.info --coverage cobertura.xml \
  --junit junit.xml --otel traces.json
```

Lens correlates exact returned trace files with SARIF, LCOV/Cobertura, JUnit,
OpenTelemetry, CODEOWNERS, Git churn, specs, ADRs, audits, lessons, and imported
facts. Provenance and completeness survive the join.

Runtime evidence may corroborate an exact structural edge. It never silently
creates graph topology.

## Export a portable review

```bash
mastermind review export --since main --out mastermind-review
```

The output contains standalone HTML, SARIF, a bounded Markdown summary, a
revision/evidence manifest, and a pinned GitHub Actions workflow. The reviewer
does not need Mastermind installed.

## Connect your coding agent

```bash
mastermind install --client all --profile core
mastermind setup cursor --scope user --write
mastermind setup continue --scope user --write
mastermind doctor --workflow --client all
mastermind workflow audit --root . --json
```

Fresh installs default to `core` (14 portable skills). `frontend` installs 19,
`security` installs 17, and `full` installs all 26. Every profile keeps the
complete Claude subagent set; only portable skill discovery is narrowed.
Updates preserve each client's installed profile unless `--profile` explicitly
changes it. Legacy schema-v1 manifests migrate as `full`. Older installers
reject the schema-v2 manifest without replacing managed files, so use the
current package for later updates.

`workflow audit` is read-only. It graphs only repository-owned source workflow
files or artifacts listed by an installed ownership manifest, then reports
missing MCP scope/registration, invalid runtime bounds, optional skill closure,
writer conflicts, unreachable tools, and componentized context estimates.

Mastermind supports Claude Code, Codex, Cursor, Continue, and generic MCP stdio
clients. Setup previews changes unless `--write` is present. The MCP server
exposes 19 non-destructive queries that may refresh the managed derived index,
8 read-only tools, and one additive write to the local gitignored scratchpad.

## Supported stack

- **Languages:** Python, TypeScript/TSX, JavaScript/JSX, Vue SFC, Rust, C#,
  Go, Java, PHP, and C/C++.
- **Platforms:** macOS arm64/x64, Linux glibc and musl arm64/x64, Windows x64.
- **Evidence:** SARIF, LCOV/Cobertura, JUnit, OTLP JSON, CODEOWNERS, Git, SCIP,
  and signed declarative facts.

The default Tree-sitter graph is syntactic and toolchain-free. Optional SCIP
adds compiler-resolved definitions and references. Dynamic dispatch,
reflection, generated code, dependency injection, re-exports, overloads, and
cross-language calls may be incomplete. Candidate tests and unreferenced
symbols are review inputs, not authorization to skip tests or delete code.

## Measured performance

On the published synthetic Rust benchmark, an Apple M3 Pro indexed 1,000 files
and 20,000 functions in a 310 ms median cold run. An unchanged scan took 41 ms.
A 10,000-file, 200,000-function corpus took 3.20 s cold and 353 ms unchanged.

These are measurements on one machine, not portable guarantees. See the
[methodology, ranges, and reproduction command](https://github.com/xcrft/mastermind/blob/main/docs/benchmarks.md).

## Documentation

| I want to… | Go here |
|---|---|
| Understand the product | [Product README](https://github.com/xcrft/mastermind) |
| Run my first review | [Getting started](https://github.com/xcrft/mastermind/blob/main/docs/getting-started.md) |
| Look up a command, limit, or MCP tool | [CLI and MCP reference](https://github.com/xcrft/mastermind/blob/main/docs/reference/mmcg.md) |
| Connect an AI client | [Client integrations](https://github.com/xcrft/mastermind/tree/main/docs/integrations) |
| Use the delivery workflow | [Review workflow](https://github.com/xcrft/mastermind/blob/main/docs/workflow.md) |
| Import external evidence safely | [Fact-ingestion SDK](https://github.com/xcrft/mastermind/blob/main/docs/fact-ingestion-sdk.md) |

The same binary is available from crates.io as
[`mmcg`](https://crates.io/crates/mmcg). npm installs it as both `mastermind`
and `mmcg`.

## License

MIT — [source and license](https://github.com/xcrft/mastermind).
