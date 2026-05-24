---
name: pr-review
description: Review a pull request for correctness, security, design issues, and operational risk — staff-engineer style. Use when the user says "review my PR", "audit this diff", "check before merge", or pastes a PR URL.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - code-review
  model: opus
  requires:
    - gh CLI (for fetching PR diffs from GitHub)
---

# PR Review

Reviews a pull request the way a staff engineer would: operational correctness and blast radius first, line-level style last. The goal is a short, prioritized list of issues, not a wall of nitpicks.

## When to use

- User says "review my PR", "audit this diff", "code review", "check before merge"
- User pastes a GitHub PR URL or a unified diff
- User asks "what's wrong with this change?"
- Do NOT use for design review of a system that doesn't exist yet — that's a different kind of review (one that doesn't yet have a paired skill in this repo).

## Prerequisites

- `gh` CLI installed and authenticated (only if reviewing from a PR URL)
- Repo checked out locally (for cross-file context)

## Steps

1. **Get the diff.** If given a URL: `gh pr diff <number>`. If given raw diff: use it as-is.
2. **Read the PR description.** What is the author trying to do? If unclear, ask — don't guess.
3. **Sort changed files by blast radius.** Migrations, auth, billing, public APIs → top. Tests, docs, internal helpers → bottom.
4. **For each file (high-blast first), check in order:**
   - **Correctness** — does it do what the description claims?
   - **Operational risk** — what happens at 10x scale? What if the network is slow? What if this runs concurrently?
   - **Security** — input validation, authz, secret handling, SQL/command injection.
   - **Error paths** — what's caught, what's swallowed, what propagates?
   - **Design** — is this the right place for this code? Does it duplicate something?
   - **Style** — only flag if it actually hurts readability.
5. **Compress findings.** Three high-confidence issues beat fifteen maybes. Drop anything you're <70% sure about.
6. **Write the report** in the format below.

## Outputs

A markdown report:

```markdown
## PR Review — <PR title>

### Must fix (blocks merge)
- **<file:line>** — <issue>. <Why it matters.> <Suggested fix in 1 sentence.>

### Should fix (before merge if possible)
- **<file:line>** — <issue>. <Why it matters.>

### Consider
- **<file:line>** — <smaller suggestion>.

### What looks good
- <1-2 specific things, not generic praise>
```

If there are no "Must fix" items, say so explicitly — silence reads as "I didn't check."

## Examples

**Input:** `gh pr 1247` — a change to the rate limiter

**Output:**
```markdown
## PR Review — Add per-tenant rate limiting

### Must fix
- **src/limiter.go:88** — Counter is incremented before the limit check, so a request that exceeds the limit still counts toward the bucket. This makes the limit effectively `N-1`. Move the increment inside the `if !exceeded` branch.
- **src/limiter.go:142** — Redis call has no timeout. If Redis is slow, every request blocks. Add a 50ms context timeout.

### Should fix
- **src/limiter.go:55** — `tenantID` is read from a header without authentication. A client can spoof another tenant's ID and consume their bucket. Pull the tenant from the authenticated session instead.

### Consider
- **tests/limiter_test.go** — No test for the concurrent-increment race. Worth adding a `t.Parallel()` test with 100 goroutines.

### What looks good
- Clean separation between the policy (limits) and the mechanism (Redis ops).
- The metrics emission at `limiter.go:201` is exactly what oncall will want.
```
