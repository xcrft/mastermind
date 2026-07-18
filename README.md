# Mastermind

<p align="center">
  <img src="docs/assets/banner.webp" alt="mastermind — circuit-board logo, xcrft/mastermind" width="720">
</p>

<p align="center">
  <a href="https://www.npmjs.com/package/@xcraftmind/mastermind"><img src="https://img.shields.io/npm/v/@xcraftmind/mastermind.svg" alt="npm version"></a>
  <a href="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml"><img src="https://github.com/xcrft/mastermind/actions/workflows/ci-mmcg.yml/badge.svg" alt="CI"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="License: MIT"></a>
  <a href="evals/benchmarks.md"><img src="https://img.shields.io/badge/evals-critic%20%2B%20auditor%20%2B%20intake-yellowgreen" alt="Evals"></a>
</p>

**Mastermind makes coding agents check their claims against your real code, not their memory.**

It's two parts:

1. **mmcg — a local codegraph.** A fast Rust indexer turns your repo into a queryable graph of symbols, calls, and imports, served to the agent over MCP. *Does `parseConfig` exist? Who calls it? What breaks if I change it?* — answered from the index, not a guess.
2. **A spec-driven agent workflow** built on it. The planner writes a spec, the executor implements it, and deterministic gates verify every claim before and after — so no hallucinated functions and no silent scope creep reach your branch.

## Quick start

Requires Node.js 24+. No Rust toolchain.

```bash
npm install -g @xcraftmind/mastermind
mastermind install                     # workflow agents + skills + MCP → Claude Code (global, once)

cd your-project
mastermind init                        # build the codegraph index for this repo
mastermind doctor                      # verify — should be all green
mastermind map . --format text         # primary deterministic project discovery
```

Restart Claude Code and ask "who calls `parseConfig`?" — the answer comes from the codegraph.

[Other install methods ↓](#install)

## mmcg — the codegraph

A Rust binary that tree-sitter-parses **nine languages** (Python, TypeScript/TSX, JavaScript/JSX, Rust, C#, Go, Java, PHP, C/C++) into a local SQLite database and answers structural questions over MCP — **22 read-only tools plus one additive local scratchpad write**:

| Ask | Tool |
|---|---|
| Does symbol X exist? | `mmcg_search` |
| What calls X? | `mmcg_callers` |
| Blast radius of changing X? | `mmcg_impact` |
| What imports this path? | `mmcg_imported_by` |
| What did this branch add vs main? | `mmcg_symbols_changed_since` |
| What will this working-tree change affect and which tests are candidates? | `mmcg_change_impact` · `mmcg_test_impact` |
| Dependency cycles? Dead code? | `mmcg_dependency_cycles` · `mmcg_unreferenced` |

All 23: `mmcg_search` · `mmcg_callers` · `mmcg_callees` · `mmcg_impact` · `mmcg_imports` · `mmcg_imported_by` · `mmcg_symbols_in_file` · `mmcg_outline` · `mmcg_files` · `mmcg_api_surface` · `mmcg_unreferenced` · `mmcg_dependency_cycles` · `mmcg_symbols_changed_since` · `mmcg_centrality` · `mmcg_map` · `mmcg_change_impact` · `mmcg_test_impact` · `mmcg_change_class` · `mmcg_recent_changes` · `mmcg_tasks` · `mmcg_status` · `mmcg_scratchpad_append` · `mmcg_scratchpad_read`. Precision notes: [`mcp/servers/mmcg/README.md`](mcp/servers/mmcg/README.md). Works with any MCP stdio client (Cursor, Continue, custom), not just Claude Code.

It's intentionally narrow — a local syntactic graph, no daemon, zero system dependencies. Twenty-two MCP tools only read the graph; `mmcg_scratchpad_append` makes an additive write to the gitignored local index for agent handoffs. Point queries are sub-millisecond; whole-graph aggregations (centrality, impact, cycles) scale with repo size. Faster and more precise than grep for "who calls what," and unlike an LSP it's trivial to snapshot and query from an agent.

`mastermind map` is the primary discovery command. Its JSON format is stable schema v1; text and Mermaid are safe projections of the same bounded response. Scope matching is lexical: `%` and `_` are literal path bytes, selected-directory component names are relative to that directory, root components remain repository-relative, and a selected file keeps its repository-relative path. Depth is 1–6 and `top` is 1–100. Work is capped at 50,000 aggregation paths, 20 languages, 20 components, 20 boundaries per component and 400 globally, 50 entry points, 100 hotspots, 50,000 scoped cycle edges, 50 cycles, and 500 cycle memberships. A `path_work_limit` reason marks partial path-derived aggregates; `top_probe` means one extra hotspot or per-component boundary row proved more results exist; `global_probe_limit` means the 401st global boundary row prevented certainty for that and later components; cycle `work_limit` means SCC analysis was skipped instead of running on truncated edges.

`mastermind impact --since REF` analyzes the committed baseline against staged, unstaged, and untracked worktree content. The schema-v1 response carries body/signature/add/remove symbol evidence, bounded caller impact, empirical component crossings, and direct/transitive/heuristic test candidates. Git output is capped at 4 MiB, changed files at 10,000, seed names at 200, graph rows at 5,000, and returned impact/tests/crossings at 500. Snapshot, root, and SHA-256 index freshness checks fail closed; `work_limit` omits an incomplete graph or heuristic slice. Focused candidates never replace the repository's full required test gate.

**More than an MCP server.** MCP is one surface; the same `mastermind` binary is the workflow's engine. The deterministic gates (`verify-spec` / `audit-spec`) read the index directly, `init` / `doctor` scaffold and health-check a project, and **miners** derive cross-repo signal the workflow can consume — e.g. `miner profile` learns your code-shape style ("write like me") into `~/.mastermind/style.md` for the planner to write in.

## The workflow

A pipeline where **the planner never implements and the executor never improvises:**

```mermaid
flowchart TB
  U([User]) --> Ref[Intake Refiner • Sonnet]
  Ref -->|clean brief| P[Planner • Opus]
  P -.->|stress-test design| C[Critic • Opus]
  P -.->|gather facts| R[Researcher • Haiku]
  P -.->|unknown-cause bug| I[Investigator • Sonnet]
  P -.->|security-sensitive scope| S[Security Auditor • Opus]
  P -->|spec| E[Executor • Sonnet]
  E -->|report| A[Auditor • Opus]
  A -.->|held / drift / broken| P
  M[(mmcg codegraph)]
  P --- M
  E --- M
  A --- M
```

The critic (before the spec) and auditor (after execution) run as independent Opus instances with no prior context — they catch the planner's own bias.

### The gates make it deterministic

Every spec is checked by Rust gates that read the mmcg index directly — a verdict can't be argued out of them:

- **`verify-spec`** (pre-execution) — every symbol in the spec exists, every file is on disk, mandatory sections are filled, FIND blocks aren't stale.
- **`audit-spec`** (post-execution) — no scope creep, no signature drift vs the pre-edit snapshot, no silently removed symbols, planned tests actually added.

The subagents query mmcg too, but that's an LLM *interpreting* results — useful, not a guarantee. **Trust the gates for correctness; trust the subagents' mmcg use for speed and reach.**

### Example: catching a hallucinated function

The executor reported:

```
[x] Added CancelOrder() to pkg/checkout/checkout.go
[x] Wired it to the existing ProcessPayment() for the refund flow
VERIFY: go test ./pkg/checkout/... — PASSED
```

The auditor ran `mmcg_search ProcessPayment` against the live index → `{ "count": 0 }`. `ProcessPayment` never existed; the executor invented a call site to it. Verdict: **contract broken** — caught by the graph, not by re-reading code.

## What's inside

```
Core
  mcp/servers/mmcg/     the mmcg core binary — codegraph (23 MCP tools, 9 languages),
                        deterministic CLI gates, and miners (author style profile)

Workflow (installed into ~/.claude/ by `mastermind init`)
  agents/subagents/     prompt-refiner · critic · researcher · investigator
                        task-executor · auditor · security-auditor
  agents/claude-md/     CLAUDE.md + CONTEXT.md templates
  skills/workflow/      task-planning · task-executor · codegraph-research
                        structured-report-contract · critical-review
                        project-map · change-impact · test-impact
                        cross-client-setup · audit-attestation
  skills/debugging/     investigation-ledger
  skills/security/      agent-security-review (OWASP ASI reference pack)
  skills/prompt-engineering/  prompt-refiner (intake gate)
  skills/coding/        no-ai-slop-comments

Proof
  evals/                adversarial eval suites — critic · auditor (real git fixtures) · intake
                        + ablation study (vanilla vs mastermind catch-rate)
```

Shared contracts — codegraph queries, the executor↔auditor report format, the investigation loop — live in skills so every agent reads one source. Non-core artifacts (pr-review, flaky-finder, doc-stub-sync, …) live in [`extras/`](extras/), not installed by default.

## Install

Ships as a **prebuilt native binary via npm** — no Rust toolchain.

**Global** (recommended)
```bash
npm install -g @xcraftmind/mastermind
mastermind setup claude --scope user --write
```
Registers `command: "mastermind"` at Claude Code user scope (writes `~/.claude.json`).

**Project-local** (version-pinned)
```bash
npm install -D @xcraftmind/mastermind
npx mastermind setup claude --scope project --root . --write
```
Writes `./.mcp.json` pointing at `./node_modules/.bin/mastermind`.

**Safe MCP setup across clients**

```text
mastermind setup <claude|cursor|codex|continue|generic> \
  --scope <project|user> [--root .] [--config PATH] [--write] [--remove] [--force]
```

Setup is a dry-run unless `--write` is present. Claude supports project JSON and user-native registration; Cursor supports project and user JSON; Codex is user-only through `codex mcp`; Continue owns one `mastermind.yaml` file at project or user scope; Generic requires an explicit `--config PATH`. Replacing or removing customized owned data requires `--force`, and file-backed customized data is copied to private `~/.mastermind/setup-backups/` storage before mutation. `mastermind doctor` parses configs as bounded data and never executes a configured command.

**One-command setup** — `npx @xcraftmind/mastermind install` copies the subagents + skills into `~/.claude/` and registers the mmcg MCP. Then run `mastermind init` in a project to build the codegraph index. `list` shows what ships; `update` refreshes the agents.

**Supported platforms:** macOS (arm64/x86_64), Linux glibc & musl/Alpine (x86_64/arm64), Windows (x86_64). Other targets: `cargo install mmcg`.

<details>
<summary>Build from source (Rust 1.96+)</summary>

```bash
cargo install mmcg                       # from crates.io
cargo install --path mcp/servers/mmcg    # from a clone
```
Same binary, `mmcg`, same subcommands.
</details>

## Commands

```bash
mastermind install               # workflow agents + skills + MCP → Claude Code (global, once)
mastermind update / list         # refresh the workflow bundle · show what ships
mastermind init                  # scaffold .mastermind/, build the index, draft CONTEXT.md
mastermind setup <client>        # preview MCP registration/removal; add --write to apply
mastermind watch                 # keep the index live as you edit
mastermind doctor                # fail-soft health checks (add --json for CI)
mastermind status / next         # task list + health · single next step
mastermind new-spec "..."        # create a task spec (--mode lite|standard|strict)
mastermind resume <task-id>      # paste-into-Claude prompt for the current phase
mastermind verify-spec <spec>    # pre-execution gate
mastermind audit-spec <spec>     # post-execution gate
mastermind audit verify <bundle> # verify sealed audit evidence against trusted inputs
mastermind audit sign <bundle>   # create a detached Ed25519 signature
mastermind miner profile         # learn your code-shape style → ~/.mastermind/style.md
mastermind uninstall --scope all # tear it down (dry-run unless --force)
```

**Stays local.** The SQLite index is built from your files by tree-sitter and never leaves your machine. `install` / `init` write the workflow bundle into `~/.claude/{agents,skills,commands}/`; `setup` updates only the selected supported client target and is dry-run-first. All configuration remains local, with no network beyond the npm registry. Fully offline: `mastermind init --no-claude --no-index --no-global`. Add `.mastermind/` to `.gitignore` — it's local working state.

## Verifiable audits and GitHub Action

`audit-spec --bundle` and `ci --bundle-dir` emit schema-v3 envelopes whose manifest is canonicalized and hashed with SHA-256. `mastermind audit verify` never accepts internal consistency alone: use an exact repository + full baseline/head + trusted root policy, a required Ed25519 signature whose key ID is allowlisted and not revoked, or both. `--integrity-only` is an explicitly untrusted diagnostic and cannot authorize PR publication.

The Docker Action and privilege-separated `workflow_run` examples are documented in [docs/github-action.md](docs/github-action.md). The PR job has read-only contents permission and no OIDC; a later workflow treats every downloaded artifact as hostile, verifies independent GitHub run/artifact evidence with a verifier bound into the trusted workflow blob, then gives attestation and PR-comment permissions to separate jobs. Executable examples contain no unresolved Action or verifier placeholders.

## Contributing

See [`CONTRIBUTING.md`](CONTRIBUTING.md) — project layout, the checks to run, and how to open a PR.

## License

MIT — see [`LICENSE`](LICENSE).
