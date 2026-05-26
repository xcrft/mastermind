# Evals

Adversarial test cases for `mastermind-critic` and `mastermind-auditor` — the two subagents whose output has clear verdict labels (`rethink`/`revise`/`ship` and `held`/`drift`/`broken`).

**This is not a coverage metric.** Each case is one regression scenario, not a guarantee.

## Files

- `critic.jsonl` — designs we want flagged (or cleanly passed)
- `auditor.jsonl` — executor reports we want verified (or caught lying)
- `runner.py` — invokes the subagent via `claude -p`, asserts on verdict + key phrases
- `fixtures/` — real-git source trees used by auditor cases; see `fixtures/<name>/README.md`

## Run

Needs `claude` CLI **and** `git` on PATH. Auth uses your Claude Code login (no API key, no per-token cost).

```bash
./evals/runner.py                                # all suites
./evals/runner.py --suite critic                 # one suite
./evals/runner.py --case c-001-slop-rethink      # one case
./evals/runner.py --model sonnet                 # default: opus
./evals/runner.py --keep-fixtures                # don't delete tmp git repos (debug)
```

Each case takes ~30–60s (Opus). Auditor cases are slightly slower because the
subagent actually runs `git diff`, `git log`, etc. on a real fixture repo.

## Auditor: real-git fixtures + live mmcg

Auditor cases reference a fixture tree under `evals/fixtures/<name>/` rather than
embedding a synthetic diff string. For each case the runner:

1. Builds a tmp git repo
2. Copies `fixtures/<name>/baseline/` → commits → tags `<baseline_ref>`
3. Replaces the working tree with `fixtures/<name>/changes/<after_ref>/` → commits → tags `<after_ref>`
4. Runs `mmcg index .` against the after-tree, leaving `.mastermind/mmcg.db` in the tmp repo
5. Passes `--add-dir <tmp>` + `--mcp-config` (mmcg stdio server pointing at the index) to `claude -p`
6. The auditor runs **real** `git diff <baseline>..<after>` AND calls live `mmcg_callers` / `mmcg_search` MCP tools against the after-tree index to compare against the spec's pre-edit snapshot

This catches failure modes synthetic diffs miss: lazy auditors that trust the
report, multi-file scope creep visible only in `git diff --name-only`, silent
file deletions, snapshot drift where the symbol's caller count changed and the
spec didn't acknowledge it.

The mmcg binary used is the in-tree `mcp/servers/mmcg/target/release/mmcg`
when available (matches the SQL schema of the SDK in this checkout), falling
back to whatever `mmcg` is on `$PATH`. Build it first with
`cargo build --release --manifest-path mcp/servers/mmcg/Cargo.toml`.

## Adding a case

### Critic case (no fixture)

```jsonc
{
  "id": "c-NNN-short-name",
  "why": "what this catches — regression scenario or golden input",
  "input": { /* domain-specific */ },
  "expect": {
    "verdict": "rethink",           // substring match, case-insensitive
    "contains": ["fabricated"],     // case-insensitive, all must be present
    "not_contains": ["ship it"]     // none may be present
  }
}
```

### Auditor case (real git fixture)

```jsonc
{
  "id": "a-NNN-short-name",
  "why": "what this catches",
  "fixture": "fake-session",        // dir under evals/fixtures/
  "baseline_ref": "baseline",       // tag name for the pre-edit commit
  "after_ref": "scope-creep",       // tag name for the executor commit; also the changes/ subdir
  "input": {
    "spec_summary": "...",
    "executor_report": "..."        // NO git_diff — auditor runs it itself
  },
  "expect": {
    "verdict": ["drift", "broken"], // string OR list (any-of)
    "contains": ["config", "scope"],
    "not_contains": ["contract held"]
  }
}
```

To add a new fixture variant: create `evals/fixtures/<name>/changes/<after_ref>/` with
the full file tree at the after-commit. The runner overlays it on baseline (deletions
work — files missing from the variant get removed in the second commit).

Rules:
- **Adversarial** (something to catch) or **golden** (something to confirm passes) — nothing else
- One scenario per case
- Phrase-match assertions only — the runner doesn't use LLM-as-judge

## When to run

- Before editing `mastermind-critic.md` or `mastermind-auditor.md`
- After editing them, to confirm dimensions still fire
- When adding a new adversarial pattern as a regression test
- After adding a fixture variant (smoke `--keep-fixtures` to inspect tmp repo)

Not on every PR. Not in CI. Hand-run by the maintainer.

## Why no LLM-judge

A judge model adds another non-deterministic layer. We assert on:
1. Verdict label (`rethink` / `held` / etc.)
2. Key phrases present / absent in reasoning

If a case passes phrase matching but the reasoning is subtly wrong, that's not caught — accepted limitation.
