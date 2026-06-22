# Evals

Adversarial test cases for `mastermind-critic`, `mastermind-auditor`, and `mastermind-prompt-refiner` — subagents whose output has clear verdict labels.

**This is not a coverage metric.** Each case is one regression scenario, not a guarantee.

## Files

- `critic.jsonl` — designs we want flagged (or cleanly passed); verdicts: `rethink`/`revise`/`ship`
- `auditor.jsonl` — executor reports we want verified (or caught lying); verdicts: `held`/`drift`/`broken`
- `intake.jsonl` — raw prompts the refiner should normalize; actions: `refined`/`passthrough`/`ask`
- `runner.py` — invokes the subagent via `claude -p`, asserts on verdict/action + key phrases
- `ablation.py` — vanilla-vs-mastermind catch-rate study over the planted-defect auditor cases (does the codegraph + auditor contract beat plain `claude -p` + grep/read?); see [Ablation](#ablation)
- `fixtures/` — real-git source trees used by auditor cases; see `fixtures/<name>/README.md`
- `benchmarks.md` — current pass-rates per suite + trust conditions + ablation pointer

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
    "verdict": "rethink",           // prose regex match, case-insensitive word boundary
    "contains": ["fabricated"],     // case-insensitive, all must be present
    "not_contains": ["ship it"]     // none may be present
  }
}
```

### Auditor case (real git fixture)

Verdict is checked against the **structured YAML tail** the auditor emits inside
`<!-- mastermind:audit-begin --> … <!-- mastermind:audit-end -->` sentinels — not prose.
If the auditor doesn't produce that block the case fails.

```jsonc
{
  "id": "a-NNN-short-name",
  "why": "what this catches",
  "fixture": "fake-session",        // dir under evals/fixtures/
  "baseline_ref": "baseline",       // tag name for the pre-edit commit
  "after_ref": "scope-creep",       // tag name for the executor commit; also the changes/ subdir
  "allow_no_mmcg": false,           // optional; default false — hard-fail if mmcg index missing
  "input": {
    "spec_summary": "...",
    "executor_report": "..."        // NO git_diff — auditor runs it itself
  },
  "expect": {
    "verdict": ["drift", "broken"], // matched against structured verdict field, string OR list
    "contains": ["config", "scope"],// secondary: prose phrases in reasoning
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

## Intake suite

5 cases covering the refiner's core behaviors:

| case | scenario | expected action |
|---|---|---|
| i-001 | vague client message with buried goal | `refined` — planner-ready prompt + NEEDS placeholders |
| i-002 | already tight request with verb/deliverable/scope | `passthrough` — returned unchanged |
| i-003 | genuinely ambiguous goal (multiple valid interpretations) | `ask` — 1-3 clarifying questions, no prompt |
| i-004 | overbroad multi-intent bundle | `refined` — primary intent isolated, others marked out-of-scope |
| i-005 | production database migration with risk signals | `refined` — strict mode, risk: high, rollback flagged as NEEDS |

Each case asserts on the structured `<!-- mastermind:intake-begin --> ... <!-- mastermind:intake-end -->` YAML block the refiner emits. If the block is absent the case fails.

## Ablation

`ablation.py` measures the **marginal value** of the codegraph + auditor contract:
does the Mastermind auditor catch defects a strong *vanilla* agent misses? For each
planted-defect auditor fixture it runs two conditions on the same git repo, scored
with the same phrase signal as the suite:

- **vanilla** — plain `claude -p` with shell access (git/grep/read) and a neutral
  senior-reviewer prompt. No mmcg, no auditor system prompt — the honest "Claude +
  grep/read" baseline, so the delta isolates the codegraph + contract, not a strawman.
- **mastermind** — the real auditor path (auditor subagent + live mmcg). Re-run
  head-to-head with `--with-mastermind`; otherwise it compares against the auditor
  suite's own result.

Golden (`held`) cases are excluded — nothing to catch. Record results in `benchmarks.md`.

```bash
python evals/ablation.py                   # vanilla over all defect cases
python evals/ablation.py --with-mastermind # both conditions, head-to-head
```

## When to run

- Before editing `mastermind-critic.md`, `mastermind-auditor.md`, or `mastermind-prompt-refiner.md`
- After editing them, to confirm behaviors still fire
- When adding a new adversarial pattern as a regression test
- After adding a fixture variant (smoke `--keep-fixtures` to inspect tmp repo)

Not on every PR. Not in CI. Hand-run by the maintainer.

## Why no LLM-judge

A judge model adds another non-deterministic layer. We assert on:
1. Verdict label (`rethink` / `held` / etc.)
2. Key phrases present / absent in reasoning

If a case passes phrase matching but the reasoning is subtly wrong, that's not caught — accepted limitation.
