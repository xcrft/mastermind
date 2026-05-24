# Evals

Adversarial test cases for `mastermind-critic` and `mastermind-auditor` — the two subagents whose output has clear verdict labels (`rethink`/`revise`/`ship` and `held`/`drift`/`broken`).

**This is not a coverage metric.** Each case is one regression scenario, not a guarantee.

## Files

- `critic.jsonl` — designs we want flagged (or cleanly passed)
- `auditor.jsonl` — executor reports we want verified (or caught lying)
- `runner.py` — invokes the subagent via `claude -p`, asserts on verdict + key phrases

## Run

Needs `claude` CLI on PATH. Auth uses your Claude Code login (no API key, no per-token cost).

```bash
./evals/runner.py                                # all suites
./evals/runner.py --suite critic                 # one suite
./evals/runner.py --case c-001-slop-rethink      # one case
./evals/runner.py --model sonnet                 # default: opus
```

Each case takes ~30s (Opus). Full suite (6 cases) ~3 min.

## Adding a case

Append one line to the matching `.jsonl`:

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

Rules:
- **Adversarial** (something to catch) or **golden** (something to confirm passes) — nothing else
- One scenario per case
- Phrase-match assertions only — the runner doesn't use LLM-as-judge

## When to run

- Before editing `mastermind-critic.md` or `mastermind-auditor.md`
- After editing them, to confirm dimensions still fire
- When adding a new adversarial pattern as a regression test

Not on every PR. Not in CI. Hand-run by the maintainer.

## Why no LLM-judge

A judge model adds another non-deterministic layer. We assert on:
1. Verdict label (`rethink` / `held` / etc.)
2. Key phrases present / absent in reasoning

If a case passes phrase matching but the reasoning is subtly wrong, that's not caught — accepted limitation.
