# Documentation

Start with the job you need to finish. The root [README](../README.md) is the
product tour; these pages take you from a first local review to exact protocol,
security, and extension contracts.

## Get a useful result

| Goal | Document |
|---|---|
| Install Mastermind and index a repository | [Getting started](getting-started.md) |
| Connect an AI client | [Claude Code](integrations/claude-code.md), [Codex](integrations/codex.md), [Cursor](integrations/cursor.md), [Continue](integrations/continue.md), or [generic MCP](integrations/generic-mcp.md) |
| Use Direct, Verified, or Strict delivery | [Workflow](workflow.md) |
| Look up a command, schema, limit, or MCP tool | [CLI and MCP reference](reference/mmcg.md) |

## Review a change or enforce a boundary

| Goal | Document |
|---|---|
| Run the local diff-first UI | [`mastermind ui` reference](reference/mmcg.md#mastermind-lens-mmcg-ui) |
| Export standalone HTML, SARIF, summary, and manifest files | [PR evidence package](reference/mmcg.md#pr-evidence-package-mmcg-review-export) |
| Verify and publish signed audit evidence | [GitHub Action](github-action.md) |
| Enforce architecture rules | [Policy CLI](reference/mmcg.md#architecture-policy-as-code-mmcg-policy-check) |
| Review architecture drift over time | [Temporal graph](reference/mmcg.md#temporal-graph-mmcg-temporal) |

## Bring in more evidence

| Goal | Document |
|---|---|
| Import SARIF, coverage, JUnit, OTLP, or custom facts | [Fact-ingestion SDK](fact-ingestion-sdk.md) |
| Add compiler-resolved definitions and references | [SCIP overlay](reference/mmcg.md#optional-scip-semantic-overlay) |
| Query several pinned local indexes | [Local team graph](team-graph.md) |

## Maintainers

| Topic | Document |
|---|---|
| Reproduce indexing measurements | [Benchmarks](benchmarks.md) |
| Build, test, and submit a change | [Contributing](../CONTRIBUTING.md) |
| Understand repository-level validators | [Scripts](../scripts/README.md) |
| Run behavioral evaluation suites | [Evals](../evals/README.md) |
| Report a vulnerability | [Security policy](../SECURITY.md) |

## Documentation rules

- Guides get a reader to an outcome. Reference pages define exact behavior and
  limits. Landing pages point to both instead of duplicating them.
- Commands must be copyable from the repository root unless a different
  working directory is stated.
- Performance claims must link to [methodology and raw parameters](benchmarks.md).
- Static graph results are evidence, not runtime proof. Every page must preserve
  that distinction where it matters.
- `docs/examples/` contains copyable configuration. CI parses those files; they
  are not pseudocode.
