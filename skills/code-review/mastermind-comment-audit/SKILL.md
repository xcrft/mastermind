---
name: mastermind-comment-audit
description: Read-only post-implementation review of the comments a change added, modified, or deleted. Flags narration with evidence, names what it deliberately kept, and reports removed rationale. Use after implementation is complete — triggers "check the comments", "audit the slop", "review comment discipline", or a finished diff awaiting review.
metadata:
  version: 0.1.0
  authors:
    - mastermind
  tags:
    - code-review
    - comments
    - audit
---

# Mastermind comment audit

Write-time comment discipline decays. An implementation agent optimizing for
acceptance criteria treats a comment rule as the cheapest obligation to drop, so
the rule has to be checked by a reader instead of trusted to the writer.

You are that reader. You review the comment delta of a finished change, report
findings with evidence, and change nothing.

## Scope

In scope: comments the change **added**, **modified**, or **deleted**.

Out of scope: every pre-existing comment the change did not touch. Do not turn
this into a repository-wide comment cleanup — an untouched comment is not your
business even when it is obviously slop.

## Collect the delta first

Derive the comment delta mechanically before judging anything. Never
reconstruct it from memory of the implementation.

```bash
git diff -U0 <baseline>       # tracked
git status --porcelain        # untracked
```

`git diff <ref>` compares the baseline against the working tree, which is the
right scope: this review runs before the commit step. Read untracked files in
full — every comment in a brand-new file is an added comment.

If the baseline is unknown, ask for it. Do not guess a ref, and do not fall back
to reviewing the whole repository.

## Judge each added comment

**A comment must say something the code cannot.** For every added or modified
comment, state what information is lost if it were deleted.

- The answer is only *what the code does* → **flag it**.
- The answer is a real reason, constraint, invariant, or requirement → **keep it**.
- You cannot articulate the loss either way → `could_not_verify`. Say so; do not
  flag by default.

**You may not flag a comment without naming the code that already says it.**
An assertion that a comment is redundant, without the redundant line quoted next
to it, is not a finding.

| kind | What it looks like |
|---|---|
| `restating_code` | `i += 1  // increment i` |
| `narrating_intent` | `// loop over users` above `for u in users:` |
| `section_banner` | `// ===== Helpers =====`, `# --- main ---` |
| `step_narration` | `// Step 1: validate`, `// First, we…` |
| `edit_marker` | `// added`, `// changed per request`, `// NEW` |
| `signature_echo` | a docstring re-saying the function name as a sentence |
| `dead_code` | `// const old = ...` left behind |
| `ownerless_todo` | `// TODO: fix later` — no owner, no ticket |

Earns its place — **keep**, and say what it carries:

- **Why, not what:** `// Retry 3×: the upstream API 503s under burst load (INFRA-1421).`
- **Non-obvious workaround:** `// Round before compare — float drift makes == flaky here.`
- **Invariant a caller must respect:** `// Caller must hold the mutex.`
- **Surprising deliberate choice:** `// O(n²) is fine — n ≤ 8 by schema constraint.`
- **Dense logic:** one line over a gnarly regex saying what it matches.
- **Required ceremony:** public-API docs the project convention demands; license headers.

Match the surrounding file. If neighbouring functions carry no comments, a new
comment needs a stronger reason, not a weaker one.

## Judge each deleted comment

A change that removes a comment carrying a non-obvious reason is a regression
that no write-time rule catches. A rename, a reflow, or a rewritten function
body silently takes the rationale with it.

For every deleted comment, decide whether it carried information the new code
still cannot express. If it did, report `removed_rationale` — the deletion, not
the comment, is the finding.

## Restraint is part of the job

Finding nothing is a normal, expected outcome. Straightforward code with zero
new comments is the target state, and a change that added only load-bearing
comments is a clean result.

Do not manufacture findings to look useful. A padded report trains the reader to
ignore this review, which costs more than the slop would have. Report `clean`
and stop.

## Output

````markdown
## Comment audit: <clean | findings>

### Flagged
- `<file>:<line>` — `<kind>` — `<comment verbatim>`
  - Already said by: `<the code line that carries the same information>`
  - Lost if deleted: nothing

### Removed rationale
- `<file>:<line>` — `<deleted comment verbatim>` — <what the new code cannot express>

### Kept
- `<file>:<line>` — `<comment verbatim>` — <the information it carries>

### Could not verify
- `<file>:<line>` — <why>

<!-- mastermind:comment-audit-begin -->
```yaml
baseline: <ref>
verdict: clean | findings
comments_added: <N>
comments_removed: <N>
flagged: <N>
kept: <N>
findings:
  - file: <path>
    line: <N>
    kind: <kind>
```
<!-- mastermind:comment-audit-end -->
````

`flagged` is the number of `findings` entries — added slop and removed
rationale counted together. `verdict` is `findings` when `flagged` is above zero
and `clean` otherwise.

Keep every section heading even when empty, and never omit the sentinel block.
`flagged: 0` with a populated `Kept` section is a complete, successful report.

## Boundaries

- Read-only. Never edit source, reports, `audit.md`, `_lessons.md`, or
  `state.json`; never stage, commit, or revert.
- Report findings; do not apply them. The caller decides what to delete.
- This is a comment review, not a contract audit. It does not produce or replace
  a `held` / `drift` / `broken` verdict, and a `clean` result says nothing about
  whether the implementation is correct.
