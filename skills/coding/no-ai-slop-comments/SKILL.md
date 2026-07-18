---
name: no-ai-slop-comments
description: Strip AI-slop comments from code — keep only comments that explain a *why* the code can't say itself, delete anything that just restates it. Use when writing or editing code (especially as an executor applying a spec) or reviewing a diff for comment noise. Triggers — "stop adding comments", "no slop comments", "too many comments", code that narrates itself.
metadata:
  version: 0.1.1
  authors:
    - mastermind
  tags:
    - coding
---

# No AI-Slop Comments

LLMs over-comment. Left unchecked, they narrate every line, banner every section,
and annotate every edit. Apply this rule to comments added or modified in the
current change. Do not turn a scoped implementation into a repository-wide
comment cleanup.

## When to use

- Writing new code, or editing existing code, in any language.
- Implementing a spec as an executor — preserve literal blocks and add comments only when the contract or code needs information the code cannot express.
- Reviewing a diff that reads like a tutorial: `// loop over users`, `# increment counter`, `// --- Helpers ---`.
- The user says "stop commenting", "no slop comments", or "you comment too much".

## The rule

**A comment must say something the code cannot.** If a comment you added or
changed loses no information when deleted, remove it.

Comments explain **why**, not **what**. The code already says what it does.

## Delete these — slop

| Category | Slop example | Why it's slop |
|---|---|---|
| Restating code | `i += 1  // increment i` | The code already says it. |
| Narrating intent | `// loop over users` above `for u in users:` | The loop says it. |
| Section banners | `// ===== Helpers =====`, `# --- main ---` | Structure isn't prose. Use functions. |
| Step narration | `// Step 1: validate`, `// First, we…` | You're talking to yourself. |
| Edit markers | `// added`, `// changed per request`, `// NEW` | Git is the changelog, not the source. |
| Echoing the signature | a docstring that re-says the function name as a sentence | More words, no information. |
| Dead code | `// const old = ...` left behind | Delete it; git remembers. |
| Ownerless TODO | `// TODO: fix later` | No owner, no ticket, no action. |

## Keep these — they earn their place

- **Why, not what:** `// Retry 3×: the upstream API 503s under burst load (INFRA-1421).`
- **Non-obvious workaround:** `// Round before compare — float drift makes == flaky here.`
- **Invariant a caller must respect:** `// Caller must hold the mutex before calling.`
- **Deliberate, surprising choice:** `// O(n²) is fine — n ≤ 8 by schema constraint.`
- **Dense-logic intent:** one line over a gnarly regex / bit-twiddle saying what it matches.
- **Required ceremony:** public-API docstrings where the project convention demands them; license headers.

If a comment carries a non-obvious reason, legal requirement, security
constraint, public contract, invariant, or tool-required documentation, keep
it. When uncertain about an existing comment outside the current diff, preserve
it and leave cleanup to an explicitly scoped review.

## Match the surrounding code

Before adding any comment, look at the file. If the neighbouring functions carry no comments, yours shouldn't either. Mirror the existing density and docstring convention — don't import a heavier commenting style than the codebase already uses.

## Before / after

```python
# BEFORE — slop
def total(items):
    # initialize sum to zero
    s = 0
    # loop through all items
    for it in items:
        s += it.price  # add price to sum
    # return the total
    return s

# AFTER
def total(items):
    return sum(it.price for it in items)
```

```python
# Keep — the comment carries what the code can't
def parse(ts):
    # Vendor sends epoch millis, not seconds — divide before fromtimestamp.
    return datetime.fromtimestamp(ts / 1000)
```

## The one-line test

Read the comment, then read the code. Did the comment tell you something the
code did not? If no, delete comments introduced by your change. If uncertainty
involves an existing comment, preserve it unless the user requested cleanup.
