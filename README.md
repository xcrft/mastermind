<p align="center">
  <img src="docs/assets/brand/mastermind-mark.svg" alt="Mastermind logo" width="96">
</p>

<h1 align="center">Mastermind</h1>

<p align="center">Repository context and evidence-backed workflows for coding agents.</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@xcraftmind/mastermind"><img src="https://img.shields.io/badge/npm-v4.0.1-CB3837?logo=npm" alt="npm version 4.0.1"></a>
  <a href="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml"><img src="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg" alt="CI status"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-4f46e5.svg" alt="MIT license"></a>
</p>

Mastermind indexes your code and documentation locally, gives coding agents
that context over MCP and prompt hooks, and checks a task's evidence before it
counts as done. It works with Claude Code and Codex natively, and with any
stdio MCP client.

## Quick start

You need Node.js 24 or later. The [npm package](npm/mastermind/package.json)
ships prebuilt binaries for macOS, Linux and Windows. Run inside your
repository:

```bash
npm install -g @xcraftmind/mastermind
mastermind init
```

`init` sets up the clients it finds, indexes the repository and saves its
choices in `.mastermind/setup.json`. Restart the client afterwards.
[Getting started](docs/getting-started.md) covers the options, mining, updates,
Windows and the [source build](docs/getting-started.md#current-source-build).

| Command | Result |
|---|---|
| `mastermind status --json` | The index, client configuration, observed sessions and miner |
| `mastermind ui --since main` | Opens Lens, the read-only change review UI, against a Git baseline |
| `mastermind update` | Updates the package in its npm scope and refreshes the installed workflows |

## What you can do

| Need | What Mastermind gives you |
|---|---|
| Review a change | Changed symbols, callers, component boundaries and candidate tests |
| Find context | Code search, cited Markdown, decisions and task history |
| Apply working preferences | Reviewed habits, selected by project, role and workflow |
| Delegate a task | Approved scope, recorded checks, audit and semantic review |
| Share evidence | Lens, offline HTML, SARIF and signed audit envelopes |
| Import analysis | SCIP, scanner reports, coverage, tests, traces and facts |

<p align="center">
  <img src="docs/images/lens/mastermind-lens-live-desktop.png" alt="Mastermind Lens showing change impact and supporting evidence" width="1000">
</p>

## Give an agent context

Each selected client gets several layers of context through prompt hooks and
MCP:

| Context | Scope |
|---|---|
| Person | Advisory habits and preferences shared across repositories |
| Project | Repository decisions, constraints and knowledge |
| Code and documentation | Indexed structure and cited sources |
| Role | Duties for this invocation |
| Workflow | Planning, execution, verification and review |
| Task evidence | Results, blockers and the reviewed revision |

Profile access is limited to the selected project and clients, and preferences
grant no permissions. `--profile-access off` turns it off, and
`--mining capture` collects without analysis. [Architecture](docs/architecture.md)
explains the layers, and [persona hooks](docs/guides/persona-hooks.md) shows how
to build and review a profile. Other clients connect through
[MCP setup](docs/integrations/generic-mcp.md).

## Take a task through review

```bash
mastermind new-spec "Add account recovery"
```

Fill in the goal, scope, acceptance criteria and checks, then follow the
[workflow guide](docs/workflow.md). Any client can implement the spec. Native
execution and review use Claude Code. A task completes only with current
evidence and a resolved review.

## Limits

| Boundary | Meaning |
|---|---|
| Static codegraph | Dynamic dispatch, reflection, generated code and cross-language calls can be missing |
| Personal profile | Reviewed preferences stay advisory |
| Task records | Local and owner-writable. They show what ran, not that the change is correct |
| Native process policy | Uses client restrictions. There is no OS sandbox |
| Task mining | Uses the current agent's allowance for the added context and tool calls. Drafts need review |
| Separate model requests | Background mining, refinement, drafting, execution and review each need their own option, see [provider contracts](docs/reference/persona.md#hook-processor-contract) |

[Language coverage](docs/reference/mmcg.md#language-coverage) includes Python,
TypeScript and TSX, JavaScript and JSX, Vue SFC, Rust, C#, Go, Java, PHP and
C/C++. Native hooks, managed mining and supervised execution need macOS or
Linux. On Windows you get local indexing and client setup with `--mining off`,
see [process supervision](docs/reference/task-runtime.md#process-supervision).

## Documentation

| Start | Reference |
|---|---|
| [Getting started](docs/getting-started.md) | [CLI and index](docs/reference/mmcg.md) |
| [Architecture](docs/architecture.md) | [MCP tools](docs/reference/mcp.md) |
| [Workflow](docs/workflow.md) | [Changelog](CHANGELOG.md) |
| [All guides](docs/README.md) | [Security policy](SECURITY.md) |

## Build and contribute

Source builds need Rust 1.96 or later, as declared in
[Cargo.toml](mcp/servers/mmcg/Cargo.toml).

```bash
cargo install --path mcp/servers/mmcg --locked
just check
```

Cargo installs `mmcg`. The npm package provides `mastermind` and `mmcg`. See
[Contributing](CONTRIBUTING.md), [GitHub Issues](https://github.com/xcrft/mastermind/issues)
and the [security policy](SECURITY.md).

## License

[MIT](LICENSE).
