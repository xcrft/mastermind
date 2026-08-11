# Contributing

Mastermind is two things: **mmcg** (a Rust codegraph binary) and a **spec-driven workflow** (the subagents + skills installed into Claude Code). Contributions to either are welcome.

## Project layout

| Path | What it is |
|---|---|
| `mcp/servers/mmcg/` | The mmcg binary — Rust crate: indexer, MCP server, CLI gates, miners. |
| `skills/` · `agents/` | The workflow artifacts (markdown + YAML frontmatter) installed by `mastermind init`. |
| `npm/mastermind/` | The npm wrapper that ships the prebuilt binary + the workflow bundle. |
| `scripts/` | `validate.py` (artifact-structure check) and friends. |
| `evals/` | Adversarial eval suites for critic, auditor, intake, and all portable workflow skills. |

## Dev setup & checks

**Rust (`mcp/servers/mmcg/`)** — run all three before opening a PR:

```bash
cd mcp/servers/mmcg
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt -- --check
cargo deny check
```

**Workflow artifacts (`skills/`, `agents/`)** — markdown with frontmatter; CI runs the structure validator, run it locally too:

```bash
python3 -m venv .venv
.venv/bin/pip install --require-hashes -r scripts/requirements.txt
.venv/bin/python scripts/validate.py
```

Exit code 0 means clean. See [`scripts/README.md`](scripts/README.md) for what it checks.

**Agent behavior** — if you change a subagent/skill prompt, run the relevant
suite (`critic`, `auditor`, `intake`, or `workflow`). Before merging a broad
workflow change, run `bash evals/run-verified.sh`; it executes deterministic
gates and then every model-backed suite. The evals need the `claude` CLI on
PATH. `validate.py` checks structure, not behavior.

## Pull requests

- Keep the change focused; describe **what** it does and **how you tested it**.
- All checks above must pass — CI enforces them.
- Commit subjects: a conventional prefix, imperative (`feat(miner): …`, `fix: …`); no long body needed.
- Breaking changes (renames, removed flags, changed behavior): open an issue first to discuss.

## Reporting bugs

Use the [bug issue template](.github/ISSUE_TEMPLATE/bug.md) — include the version (`mmcg --version`), what you expected, what happened, and your OS.

## Code of conduct

See [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).
